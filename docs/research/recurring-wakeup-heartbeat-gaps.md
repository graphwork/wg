# Recurring Wakeups, Cron Loops, and Heartbeat Conflicts — Research Note

> Task: `study-recurring-wakeup-heartbeat-gaps`
> Date: 2026-07-05
> Scope: Why long-running autonomous WG processes are unreliable (Braydon's
> 30–50% success rate, weekly Monday job-search not firing, external heartbeat
> "fighting" the agent).

## 1. Executive summary

WG has **two** recurring mechanisms today, neither of which is a durable,
host-independent scheduler:

1. **Cron tasks** (`src/cron.rs`, `Task.cron_schedule`/`cron_enabled`/
   `last_cron_fire`/`next_cron_fire`) — wall-clock expressions evaluated by the
   *running* dispatcher tick.
2. **Cycles** (`src/cycle.rs`, `Task.cycle_config`, `--max-iterations`,
   `--cycle-delay`) — iteration-based loops with optional `ready_after` delay.

Both are **parasitic on the dispatcher tick**, which only exists while
`wg service` (the daemon) is alive in a user process. There is **no
system-level persistence**: no systemd unit, no launchd plist, no `@reboot`
hook, no host cron entry that starts the daemon. `scripts/setup-nightly-cleanup.sh`
itself just calls `wg add --cron …`, i.e. it bootstraps a WG-cron task that
*also* needs the WG daemon awake to fire — the recursive reliability hole.

The Monday job-search failure and the "heartbeat fights the agent" symptom are
both explained by this single architectural fact plus a small number of
concrete bugs (§4). The missing product semantics (§5) are the larger gap:
catch-up, timezone/DST, durability across reboots, and a clean external-trigger
contract do not exist yet.

## 2. Mechanism map (what exists today)

| Surface | File:line | What it does | Trigger source |
|---|---|---|---|
| Cron parse/fire/jitter | `src/cron.rs` (`parse_cron_expression`, `calculate_next_fire`, `calculate_next_fire_with_jitter`, `is_cron_due`, `reset_cron_task`) | Parse 5/6-field cron (UTC), compute next fire ±jitter, decide due | dispatcher tick |
| Task cron fields | `src/graph.rs:451,539…` (`cron_schedule`, `cron_enabled`, `last_cron_fire`, `next_cron_fire`, `not_before`, `ready_after`, `paused`, `wait_condition`) | Per-task scheduling state | dispatcher tick + CLI |
| Cron reset phase | `src/commands/service/coordinator.rs:4595` (Phase 2.95) | Reopen Done cron tasks, recompute `next_cron_fire` from `now` | dispatcher tick |
| Cron due gate (ready) | `src/query.rs:8–32` (`is_time_ready`) | A cron task is `ready` only when `is_cron_due` returns true | dispatcher tick |
| Cron task creation | `src/commands/service/ipc.rs:1213–1317` and `src/commands/add.rs` (`--cron`) | Validate expression, set `next_cron_fire = next(now)` | CLI / IPC add |
| Cycle re-activation | `src/graph.rs:2579` (`reactivate_cycle_members`); `src/commands/service/coordinator.rs` Phase 2.5 | Reopen Done cycle members, set `ready_after = now + delay`, bump `loop_iteration` | dispatcher tick |
| Cycle failure restart | `src/commands/service/coordinator.rs:4567` (Phase 2.6) | Reopen cycle members when a member Failed and `restart_on_failure` | dispatcher tick |
| Waiting tasks | `src/commands/service/coordinator.rs:477` (`evaluate_waiting_tasks`) | Resolve `wait_condition` (`wg wait`) | dispatcher tick |
| Tick loop / poll interval | `src/commands/service/mod.rs:2154` (`run_daemon`), `:2618` (sleep calc), `default_poll_interval = 5s` (`src/config.rs:3949`) | The single heartbeat of the whole system | daemon process |
| Agent liveness heartbeat | `src/commands/heartbeat.rs`, `src/service/registry.rs` (`last_heartbeat`, `AgentStatus::Dead`) | Agents `wg heartbeat <agent-N>`; stale → reaped by `wg sweep`/dead-agents | worker agents themselves |
| Graph-watcher wake | `src/commands/service/mod.rs:2618…` (self-pipe poll) | fs change → schedule settled tick | graph writes |
| Nightly cleanup bootstrap | `scripts/setup-nightly-cleanup.sh`, `scripts/nightly-cleanup.sh` | `wg add --cron "0 0 2 * * *"` cleanup task | (recursive) dispatcher tick |

### 2.1 The one heartbeat that exists is NOT what Braydon means

`wg heartbeat` (`src/commands/heartbeat.rs`) is **agent-process liveness**, not
a task/recurrence trigger. Workers call `wg heartbeat agent-N` to update
`AgentRegistry.last_heartbeat`; `wg sweep` / `dead_agents` / the dispatcher's
stuck-task phase reap agents whose heartbeat is older than
`config.agent.heartbeat_timeout` (default 5 min). An **external** heartbeat
(e.g. a host cron that pokes WG every minute) has no defined contract with this
layer — see §4.4 and §5.4.

## 3. End-to-end path of "weekly Monday job-search"

Intended setup: `wg add "Monday job search" --cron "0 9 * * 1" -d "…"`.

1. `wg add` → IPC `add` (`src/commands/service/ipc.rs:1213`) validates the
   expression and sets `next_cron_fire = calculate_next_fire(schedule, now)`
   — the **next** Monday 09:00 **UTC** (see §4.2). Task is `Open`.
2. `is_time_ready` (`src/query.rs:31`) returns false until `next_cron_fire <= now`.
3. When due, the dispatcher's ready check admits it, `build_auto_assign_tasks`
   spawns a worker.
4. Worker calls `wg done` → task `Done`.
5. Next tick, Phase 2.95 (`coordinator.rs:4595`) calls `reset_cron_task`
   (`src/cron.rs:reset_cron_task`): sets `last_cron_fire = now`,
   `next_cron_fire = next_fire_with_jitter(now)`, status `Open`. The task is
   now `Open` but `is_time_ready` is false again (next fire is in the future),
   so it waits another week.

Every step requires the daemon to be running a tick at the right moment.

## 4. Concrete failure hypotheses (likely bugs vs semantics gaps)

### 4.1 [BUG] Missed ticks during daemon downtime are dropped, not caught up
`reset_cron_task` computes `next_cron_fire` **from `now`** when it reopens the
task (`src/cron.rs:reset_cron_task`, line ~`task.next_cron_fire =
calculate_next_fire_with_jitter(&task.id, &schedule, now)`). So if the daemon
was down across Monday 09:00 and restarts Tuesday, the task reopens with
`next_cron_fire = next Tuesday 09:00` — **the missed Monday run is silently
skipped forever**. There is no `missed_cron_fires` counter, no catch-up policy,
no "fire N times then resume" semantic. This is the single most likely cause
of "the weekly job didn't trigger" when the laptop was asleep / the daemon
wasn't running at the fire instant.

### 4.2 [SEMANTICS GAP] Cron is UTC-only; no timezone or DST handling
`parse_cron_expression` feeds the expression straight to the `cron` crate with
`DateTime<Utc>`. `is_cron_due` compares in UTC. A user who writes
`0 9 * * 1` thinking "Monday 9am local" actually gets Monday 09:00 **UTC**,
and DST transitions shift the local fire time by an hour twice a year. For a
weekly Monday job this means some weeks it fires at the "wrong" local hour and,
depending on the host's UTC offset, it may even appear to fire on Sunday. No
`--tz`/`timezone` field exists on `Task`; the `cron` crate supports `TimeZone`
but WG never passes one. (See `src/cron.rs:parse_cron_expression`.)

### 4.3 [BUG/SEMANTICS] `is_cron_due` has no "last fire" catch-up when `next_cron_fire` is unset
`is_cron_due` (`src/cron.rs:is_cron_due`): if `next_cron_fire` is present it
gates purely on `next_cron_fire <= now`. If `next_cron_fire` is `None`/invalid
it falls to either `schedule.includes(now)` (when `last_cron_fire` is `None`)
or `calculate_next_fire(schedule, last_fire) <= now`. The
`schedule.includes(now)` branch only returns true if `now` matches a cron
tick **exactly** — with a 5s poll interval and a minute-granularity cron this
is usually fine, but for a *weekly* cron (`0 9 * * 1`) the matching window is
one minute per week; if the daemon happens to be mid-restart, settling, or
paused (zero-output backoff, `coordinator.rs:4669`) during that minute, the
fire is missed and, per §4.1, never recovered. The `next_cron_fire` path is
robust; the fallback path is fragile. We should make `next_cron_fire`
authoritative and never rely on `includes(now)`.

### 4.4 [BUG] External heartbeat has no contract and actively fights the dispatcher
There is no documented external-trigger API. A host cron that does
`wg heartbeat <id>` for an ID it doesn't own is rejected (`heartbeat.rs:run_auto`
bails on non-`agent-N` IDs), but a host cron that does any of the following
*will* race the dispatcher:
- `wg add ...` to re-create a task each tick → graph churn, duplicate nodes,
  the dispatcher's `resurrect_done_tasks` (`coordinator.rs:627`) and
  `unblock_stuck_tasks` (`coordinator.rs:1141`) reactivate/reassign in
  conflict with the external trigger's intent.
- `wg done <id>` to force completion → fights the worker that's still running;
  the dispatcher's cycle/cron reset phases then reopen it.
- Editing the graph JSON directly while a tick holds the session lock →
  `session_lock.rs` serializes, but the external writer loses to the
  dispatcher's `modify_graph` atomic save.

"The wg agent would fight it" most plausibly means: the external heartbeat
writes to the graph/registry, the next dispatcher tick (5s) reverts or
reinterprets those writes, and the two loop forever. There is no
"external-trigger-only" path that the dispatcher respects as authoritative.

### 4.5 [SEMANTICS GAP] No durability across host reboot / no auto-restart of the daemon
`grep` for `systemd`/`launchd`/`@reboot` across `src/`, `scripts/`, `docs/`
returns nothing relevant. The daemon restarts itself on binary change
(`service/mod.rs:3130`) but **not** on host reboot. If the laptop reboots, the
daemon is dead until a human (or a host cron) starts it again. WG cron tasks
cannot wake the daemon because the daemon is the thing that fires them. This
is the structural reason WG "keeps falling back to cron-style triggers" —
Braydon's fallback is *host* cron, which is the only durable scheduler in the
stack.

### 4.6 [BUG] Zero-output / provider-health backoff can suppress a due cron task
Phases 5.5/5.6 (`coordinator.rs:4669`, `:4683`) return early from the tick
without spawning when `should_pause_spawning` is true (global API outage /
provider health failures). The cron task is correctly marked `ready` but never
assigned; on the next tick `reset_cron_task` does **not** reopen it (it's not
Done), so it sits `Open` past its fire time. When the outage clears, the task
is still `Open` and `is_cron_due` still returns true (next_cron_fire is in the
past), so it *will* eventually fire — but the run is silently delayed by the
outage window with no diagnostic. For a weekly task, a Monday-morning outage
shifts the run to "whenever the API comes back", which reads as "didn't fire on
Monday".

### 4.7 [SEMANTICS GAP] No diagnostic when a cron fire is skipped or delayed
`reset_cron_task` only eprintln-logs the reopen. There is no event recorded to
`task.log`, no `wg show` field for "missed fires", no `wg metrics` counter for
"cron fires missed vs fired", and no `wg status` line for "next cron fire
across the graph". `wg list` shows `cron_enabled`/`cron_schedule`/
`next_cron_fire` (`src/commands/list.rs:94`) but nothing about whether a fire
was late. A user has no way to ask "did my Monday job try to fire and fail?"
See §6 for the minimal diagnostics surface.

### 4.8 [SEMANTICS GAP] Graph edits / archive can silently kill a cron task
`reset_cron_task` reopens any `Done` cron task unconditionally (it does not
check `archived` tag, `resurrect:false`, or `paused`). Conversely, a cron task
tagged `archived` by `wg chat archive` / `wg service purge-chats` is still a
`cron_enabled` task; the only thing keeping it from re-firing is that the
coordinator-agent supervisor exits for archived chats. For non-chat cron tasks
there is no archive guard in `reset_cron_task`. So archiving a cron task does
not stop it from reopening on the next tick. (Mirrors the chat-archive guard at
`coordinator_agent.rs:855` but is missing in the cron path.)

## 5. Missing framework semantics (the bigger picture)

These are **product gaps**, not bugs, and map directly to Braydon's mental
model of "different portions of a brain coordinating across areas over time":

1. **Durable host-level trigger.** WG needs a system scheduler entry
   (launchd plist on macOS, systemd timer on Linux, or a documented "host cron
   that runs `wg service ensure-running`") so the daemon is alive when a cron
   task is due. Without this, no in-WG cron is reliable.
2. **Catch-up / missed-tick policy.** Per-task configurable: `catchup = skip
   | fire-once | fire-n | fire-all`. Default `skip` (current) is wrong for
   "weekly job search" — `fire-once` is what users expect.
3. **Timezone + DST.** Add `Task.cron_timezone: Option<String>` (IANA name);
   pass to the `cron` crate via `Schedule::from_str(...).upcoming_owned(tz)`.
   Store `next_cron_fire` in UTC but compute in the named zone.
4. **External-trigger contract.** A first-class `wg poke <task>` (or
   `wg trigger <task>`) that is an *authoritative* external wake — records a
   `trigger_source = external:<name>` event, sets `ready_after = now`, and is
   NOT reverted by the dispatcher's resurrect/unblock phases. This replaces
   the "host cron writes to the graph" anti-pattern that fights the agent.
5. **Gates / decision trees over days–weeks.** Today's gates are: `not_before`
   (one-shot), `ready_after` (one-shot delay), `wait_condition` (`wg wait`,
   evaluated each tick), cycle `guard` (`IterationLessThan`), and the
   auto-evaluate/FLIP gates. There is no first-class "decision node" that an
   agent resolves to choose the *next* branch of a multi-week process. Cycles +
   `wg wait` can approximate it but the branching is implicit in agent code,
   not in the graph. This is the "graph-based, non-linear, domain-independent
   process" Braydon is asking for — see downstream task
   `design-durable-recurring-process-graphs`.
6. **Diagnostics surface.** `wg status` should show next-fire across all cron
   tasks; `wg show <cron-task>` should show fire history (last N fires,
   missed-count, last delay); `wg metrics` should track cron fires/misses.

## 6. Code touchpoints for fixes (no secrets)

| Fix | File(s) | Function / area |
|---|---|---|
| Catch-up policy | `src/cron.rs` (`reset_cron_task`, new `apply_catchup_policy`), `src/graph.rs` (new `Task.cron_catchup` + `cron_missed_fires` fields), `src/commands/service/coordinator.rs:4595` (Phase 2.95 must consult missed count) | Reopen with catch-up |
| Timezone | `src/cron.rs` (`parse_cron_expression`, `calculate_next_fire*`, `is_cron_due` — thread a `chrono_tz::Tz`), `src/graph.rs` (new `cron_timezone`), `src/commands/add.rs` (`--cron-tz`), `src/commands/service/ipc.rs:1213` | TZ-aware fire |
| Authoritative `next_cron_fire` | `src/cron.rs:is_cron_due` (drop the `includes(now)` fallback; if `next_cron_fire` missing, compute from `last_cron_fire` then from schedule, never `includes(now)`) | Robust due check |
| Archive guard in cron reset | `src/cron.rs:reset_cron_task` (skip if `tags.contains("archived")` or `paused`), mirroring `coordinator_agent.rs:855` | Don't reopen archived cron |
| External trigger contract | new `src/commands/trigger.rs` (`Commands::Trigger` in `src/cli.rs`), record `trigger_source` in `Task.log`; ensure `resurrect_done_tasks`/`unblock_stuck_tasks` skip tasks with a pending external trigger | Stop the fight |
| Daemon auto-start | new `scripts/install-wg-launchd.sh` / `scripts/install-wg-systemd.sh` writing a `~/Library/LaunchAgents/…plist` or `~/.config/systemd/user/wg.service` running `wg service start`; document in `docs/ops/runbook.md` | Host durability |
| Cron diagnostics | `src/commands/list.rs` (next-fire column), `src/commands/show.rs` (fire history), `src/commands/status.rs` (next-fire summary), `src/metrics.rs` (cron fires/misses counters), `src/cron.rs` (record a `LogEntry` on every fire AND every missed-tick detection) | Observability |
| Skip-on-paused audit | `src/commands/service/coordinator.rs:4669`/`:4683` — when spawning is paused, record why on the due cron task's log so `wg show` surfaces "delayed by API outage 12 min" | Delay visibility |

## 7. Minimal repros / smokes (before & after fixes)

All scenarios should live under `tests/smoke/scenarios/` and be listed in
`owners` of `tests/smoke/manifest.toml` (grow-only). Each must be
credential-free (use the deterministic worker / `--exec echo` path).

1. **`cron_missed_tick_catchup.sh`** (covers §4.1, Validation item "weekly
   Monday cron/wakeup behavior")
   - Create a cron task `--cron "*/2 * * * *"` (every 2 min) with a fake clock
     or `--cron-tz UTC`.
   - Let it fire once → `Done`.
   - Stop the daemon, advance a mock clock past TWO fire windows (use
     `WG_NOW` env override or a `chrono::now` injection seam — add a
     `WG_FAKE_NOW` test helper in `src/cron.rs` behind a `#[cfg(test)]`/env
     gate), restart the daemon.
   - **Before fix:** exactly one fire happens, two missed windows silently
     dropped. **After `catchup=fire-once`:** one catch-up fire, then resume
     schedule. Assert via `wg show --json` `cron_missed_fires` and the task log.

2. **`cron_timezone_dst.sh`** (covers §4.2)
   - Create `--cron "0 9 * * 1" --cron-tz America/Chicago`.
   - Drive the clock across a DST boundary (mock clock).
   - Assert `next_cron_fire` stays 09:00 Chicago (not 09:00 UTC, not shifted
     by an hour). **Before:** shifts/uses UTC. **After:** stable local time.

3. **`cron_archive_does_not_reopen.sh`** (covers §4.8)
   - Create a cron task, `wg chat archive`/tag it `archived`, mark `Done`.
   - Run one dispatcher tick. **Before:** task reopens (`Open`). **After:**
     stays `Done`/archived.

4. **`external_trigger_does_not_fight_dispatcher.sh`** (covers §4.4, the
   heartbeat/external-trigger conflict)
   - Start the daemon, spawn a long-running task (`--exec "sleep 60"`).
   - From an "external" process, run `wg trigger <task>` (the new contract) and
     concurrently let the dispatcher tick.
   - Assert the trigger event is recorded once, the dispatcher does NOT revert
     it, and there is no duplicate task / no resurrect loop. **Before (using
     `wg add`/graph edits as the trigger):** resurrect/unblock phases fight the
     external write. **After:** single authoritative trigger.

5. **`cron_diagnostics_visible.sh`** (covers §4.7)
   - Create a cron task, let it fire once, then simulate a missed tick (mock
     clock past fire while daemon down).
   - Assert `wg status` shows next-fire, `wg show <task>` shows
     `cron_missed_fires >= 1` with a log entry naming the missed window, and
     `wg metrics` shows `cron_fires_missed` incremented. **Before:** no
     observability. **After:** all three surfaces populated.

6. **`daemon_autostart_after_reboot.sh`** (covers §4.5; host-level, may need a
   containerized reboot harness)
   - Install the launchd/systemd unit, "reboot" the harness, assert `wg service`
     is alive within N seconds and a pending cron task fires on schedule.

## 8. Separation: bugs vs missing semantics

**Bugs (fix in current code, no new model):**
- §4.1 missed ticks dropped (catch-up logic in `reset_cron_task`)
- §4.3 `includes(now)` fallback in `is_cron_due` is fragile
- §4.4 external trigger has no contract and races the dispatcher
- §4.6 paused-spawning silently delays cron fires with no log
- §4.8 archived/paused cron tasks are reopened by `reset_cron_task`

**Missing framework semantics (need design — downstream tasks
`design-durable-recurring-process-graphs` and `impl-recurring-heartbeat-diagnostics`):**
- §4.2 timezone/DST support
- §4.5 host-level durability (launchd/systemd unit + `wg service ensure-running`)
- §5.1–§5.6 durable host trigger, catch-up policy model, external-trigger
  contract, decision-tree/gate semantics for multi-week processes, and the
  diagnostics surface

## 9. Recommendations for downstream implementation tasks

The two already-open dependents cover the right split:

- **`impl-recurring-heartbeat-diagnostics`** should take the **bug fixes +
  diagnostics surface** from §6 (catch-up, TZ, archive guard, authoritative
  `next_cron_fire`, `wg trigger`, `wg status`/`show`/`metrics` cron surfaces).
  Validation = smokes 1, 3, 4, 5 above.
- **`design-durable-recurring-process-graphs`** should take the **framework
  semantics** from §5 (host durability, catch-up policy model, external-trigger
  contract, decision-tree gates, multi-week process model). It should produce
  an ADR + a `Task` field spec + smokes 2 and 6.

Suggested ordering: design first (it pins the `Task` schema additions the impl
task will write), then impl. Add a `Prereq:` edge from
`impl-recurring-heartbeat-diagnostics` to `design-durable-recurring-process-graphs`
if the impl task starts touching `Task.cron_*` schema before the design lands.

## 10. Open questions to confirm with Braydon

- Is the "heartbeat" he set up (a) a host cron poking WG, (b) a `wg heartbeat
  agent-N` loop he ran himself, or (c) a separate watchdog process? §4.4
  assumes (a)/(c); (b) is harmless unless he targeted an agent ID the daemon
  also manages.
- Was the Monday job-search a `wg add --cron` task or a host crontab entry that
  ran `wg add`? The failure signature differs: the former fails per §4.1/§4.5;
  the latter fails if the daemon wasn't already running to pick up the spawned
  task.
- Does "30–50% success" cluster around laptop-sleep windows, API outages, or
  reboots? That disambiguates §4.1 vs §4.5 vs §4.6.
