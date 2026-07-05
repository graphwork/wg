# Repro: Weekly Cron Wakeup & Heartbeat Fighting Failures

Task: `repro-weekly-wakeup-heartbeat`

This document records the concrete reproduction coverage built for WG
recurring wakeups and heartbeat conflicts, plus the precise failure
findings converted into acceptance criteria for downstream tasks.

## Coverage built

| # | Artifact | What it pins |
|---|----------|--------------|
| 1 | `tests/smoke/scenarios/cron_weekly_wakeup_becomes_ready.sh` | weekly cron wakeup + missed-trigger catch-up across daemon downtime |
| 2 | `tests/smoke/scenarios/cron_paused_state_skips_dispatch.sh` | paused / waiting state interaction with recurring triggers |
| 3 | `tests/smoke/scenarios/heartbeat_external_interop_no_resurrect.sh` | external `wg heartbeat` ↔ service-reaper interop |
| 4 | unit tests in `src/cron.rs` | weekly Monday dow semantics, UTC-only timezone/DST contract, missed-trigger catch-up, **non-standard dow mapping** |
| 5 | `tests/smoke/manifest.toml` | all three scenarios registered with `owners = ["repro-weekly-wakeup-heartbeat"]` (grow-only manifest) |

All scenarios are **credential-free** (no LLM keys, no network). They use
`wg init -x shell`, `max-agents=0` (no worker spawn), and direct
`graph.jsonl` / `registry.json` rewriting via `python3` to simulate
past/future fire times and live/dead PIDs.

## Scenarios reproduced / falsified

### ✅ Weekly cron wakeup (daemon active) — WORKS
A cron-enabled task with `next_cron_fire` in the past surfaces in
`wg ready` after a coordinator tick. The gate (`is_time_ready` →
`is_cron_due` → `next_fire <= now`) holds. A future `next_cron_fire`
correctly keeps the task NOT ready.

Pinned by `cron_weekly_wakeup_becomes_ready.sh` test 1 + 2.

### ✅ Missed-trigger catch-up across daemon downtime — WORKS
If the daemon was STOPPED across the scheduled fire time
(`next_cron_fire` already in the past at restart), the FIRST tick after
restart wakes the task. The missed fire is fired LATE, not silently
dropped. `is_cron_due` returns true whenever `next_cron_fire <= now`,
independent of how long ago the fire was.

Pinned by `cron_weekly_wakeup_becomes_ready.sh` test 3.

**Caveat (acceptance criterion for downstream):** there is no
"missed-fire" audit record. A weekly fire crossed while the daemon is
down for 6 days fires once on restart — there is no catch-up for the
intermediate fires the schedule would have produced, and no log entry
naming the missed slot. `reset_cron_task` (Phase 2.95) advances
`next_cron_fire` to the NEXT schedule slot after the task completes, so
at most one fire is "merged" — acceptable for weekly cadence, but a
daily cron down for 3 days fires once, not three times. Document and
decide in `design-durable-recurring-process-graphs`.

### ✅ Paused / waiting state interaction — WORKS (gates hold)
A DUE cron task that is `paused=true` is NOT ready
(`ready_tasks_with_peers_cycle_aware` filters `task.paused` before
`is_time_ready`). A DUE cron task in `Waiting` status is NOT ready
(the `Open|Incomplete` status filter holds). An operator who pauses a
weekly cron for a maintenance window will NOT have it auto-dispatched
at fire time.

Pinned by `cron_paused_state_skips_dispatch.sh`.

### ✅ External heartbeat interop — WORKS (no resurrection)
`wg heartbeat agent-N` on a LIVE agent (real PID) refreshes
`last_heartbeat` and the agent survives the next coordinator tick
(legitimate keep-alive). `wg heartbeat agent-N` on a DEAD agent (PID
gone) does NOT resurrect it: `triage::detect_dead_reason` checks
`is_process_alive(pid)` BEFORE heartbeat, so a fresh heartbeat for a
gone process is ineffective and the next tick reaps it
(`ProcessExited`). The registry uses `flock(LOCK_EX)` on
`load_locked`, so external heartbeat writes and the daemon's tick
writes serialize — no clobbering.

Pinned by `heartbeat_external_interop_no_resurrect.sh`.

### 🐛 DISCOVERED FAILURE: non-standard day-of-week mapping
**This is the headline finding.** The `cron` crate (0.12.x) maps the
day-of-week field as **1=Sunday, 2=Monday, …, 7=Saturday** — the
opposite of standard cron (0=Sunday, 1=Monday). A user who writes
`0 0 9 * * 1` intending "every Monday 09:00" gets **Sunday 09:00** —
the weekly trigger fires on the WRONG DAY.

Pinned by the `cron_dow_mapping_is_nonstandard_one_indexed_sunday`
unit test in `src/cron.rs`, which asserts the surprising mapping at
the source level so a future crate upgrade or a local remapping layer
that aligns to standard cron flips the test and is forced to be
intentional.

### 📌 DOCUMENTED GAP: timezone / DST (UTC-only)
`is_cron_due` takes `DateTime<Utc>` and the cron crate schedules in
UTC. There is no local-timezone cron mode. A user who writes
`0 0 9 * * 2` expecting "every Monday 09:00 local" gets 09:00 UTC — a
different local wall-clock under any non-UTC zone, and one that shifts
by an hour across DST transitions without the expression changing.

Pinned by the `cron_evaluates_in_utc_no_local_dst_shift` unit test.
Not a bug to fix here — pinned so a future TZ-aware cron feature is
intentional.

## Acceptance criteria for downstream tasks

The findings above convert into precise acceptance criteria for the
downstream consumers:

### `impl-recurring-heartbeat-diagnostics`
- Surface the **non-standard dow mapping** loudly: `wg add --cron
  "0 0 9 * * 1"` and `wg list`/`wg show` output must name the day the
  expression will actually fire (e.g. `[cron: 0 0 9 * * 1 → Sunday
  09:00 UTC]`), so a user scheduling "Monday" does not silently get
  "Sunday".
- Add a `wg cron doctor` / diagnostic that lists every cron-enabled
  task with: schedule, resolved next-fire (UTC), resolved weekday,
  last-fire, and whether the task is currently due — so missed-fire
  and wrong-day states are diagnosable without reading graph.jsonl.
- Log a `cron_fire_missed` audit entry when `is_cron_due` returns
  true and `last_cron_fire` is more than one period behind
  `next_cron_fire` (the downtime catch-up case), naming the task and
  the elapsed-since-scheduled delta.

### `design-durable-recurring-process-graphs`
- Decide the missed-fire semantics: fire-once-on-restart (current) vs.
  catch-up-each-missed-slot. Current behaviour merges N missed weekly
  fires into 1; document whether that is the durable contract or
  whether a backfill mode is needed for higher-cadence crons.
- Decide the timezone contract: UTC-only (current, pinned) vs.
  TZ-aware cron (store an explicit zone, fire at fixed local
  wall-clock that does not shift across DST). The UTC-only contract is
  pinned by `cron_evaluates_in_utc_no_local_dst_shift` so a change is
  intentional.
- Decide whether `1=Sunday` is the durable dow contract or whether WG
  should remap on input to match standard cron user expectations.

### `.flip-repro-weekly-wakeup-heartbeat`
This document + the pinned unit/smoke artifacts are the fidelity
target. The repro coverage proves the structural contracts (weekly
gate, catch-up, paused/waiting gates, heartbeat-no-resurrect) and
surfaces two real findings (non-standard dow, UTC-only) for the FLIP
to probe.
