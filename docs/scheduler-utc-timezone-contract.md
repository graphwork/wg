# WG Scheduler Timezone Contract — UTC-only (durable)

> Status: **DURABLE CONTRACT** (pinned, not incidental)
> Owners: scheduler/cron subsystem (`src/cron.rs`, `src/query.rs`,
> `src/commands/service/coordinator.rs` Phase 2.95)
> Pinned by: `cron::tests::cron_evaluates_in_utc_no_local_dst_shift`
> (`src/cron.rs`), `tests/smoke/scenarios/cron_weekly_wakeup_becomes_ready.sh`,
> `tests/smoke/scenarios/cron_recurring_no_duplicate_fire.sh`
> Origin: `docs/research/recurring-wakeup-heartbeat-gaps.md` §4.2,
> `docs/repro-weekly-wakeup-heartbeat.md` ("DOCUMENTED GAP: timezone / DST").

## The contract

**WG cron expressions evaluate in UTC, and only UTC.** This is an
intentional, durable contract — not a bug to "fix" with a local-timezone
mode. Specifically:

1. `parse_cron_expression` (`src/cron.rs`) feeds the expression to the
   `cron` crate against `DateTime<Utc>`.
2. `is_cron_due(task, now: DateTime<Utc>)` compares `next_cron_fire <= now`
   entirely in UTC.
3. `calculate_next_fire` / `calculate_next_fire_with_jitter` return
   `DateTime<Utc>`.
4. `next_cron_fire` and `last_cron_fire` on `Task` are stored as RFC3339
   UTC strings (`...Z`).
5. The dispatcher tick, `wg ready`, and `wg list`/`wg show` all reason in
   UTC.

There is **no `--tz` / `Task.cron_timezone` field**, and adding one is a
deliberate schema change — not a silent fix. The UTC-only behaviour is
pinned by `cron_evaluates_in_utc_no_local_dst_shift` so that any future
move to TZ-aware cron flips the test and is forced to be intentional.

## Why UTC-only (and why that is the safe default)

- **Host-independence.** WG cron is parasitic on the dispatcher tick
  (`docs/research/recurring-wakeup-heartbeat-gaps.md` §2); the daemon may
  run on a laptop, a server, or a container, each with a different local
  zone and DST regime. A UTC schedule fires at the same instant everywhere
  and does not shift when a host's zone or DST changes.
- **No DST surprises.** A local-timezone cron shifts by an hour twice a
  year under DST. For a weekly Monday job, "9am local" silently becomes
  "8am local" or "10am local" on the DST boundary — exactly the class of
  "the weekly job didn't fire when I expected" failure this contract
  exists to prevent. UTC has no DST.
- **Auditable.** Every fire instant is an unambiguous absolute time. Logs,
  `wg show` `next_cron_fire`, and the verdict/identity content-addressing
  substrate all speak UTC already; a local zone would require a zone tag
  on every stored timestamp and reintroduce the comparison ambiguity.

## What this means for users

- When you write `wg add "Monday job search" --cron "0 0 9 * * 2"`, that
  is **Monday 09:00 UTC**, not 09:00 in your local timezone. To fire at a
  local wall-clock, convert your local time to UTC at authoring time and
  write the UTC instant into the expression.
  - Example: "every Monday 09:00 America/Chicago" (CST = UTC-6 in winter,
    CDT = UTC-5 in summer). The UTC-only contract means the **instant** is
    fixed; the *local* wall-clock it corresponds to shifts with DST. To
    keep a fixed local wall-clock across DST you would need TZ-aware cron
    (see "Future direction" below) — which WG does not currently provide.
- **Day-of-week mapping is non-standard.** The `cron` crate (0.12.x) maps
  `dow=1 → Sunday, dow=2 → Monday, …, dow=7 → Saturday` — the opposite of
  standard cron (`0=Sun, 1=Mon`). So `0 0 9 * * 2` is **Monday** in WG,
  not Sunday. This is pinned by
  `cron::tests::cron_dow_mapping_is_nonstandard_one_indexed_sunday`. A
  user scheduling "Monday" must use `2`, not `1`.

## Related scheduler-reliability contracts (pinned alongside this doc)

These are the near-term scheduler/wakeup reliability fixes that
`impl-recurring-wakeup-reliability` landed alongside this UTC contract.
All are pinned by smoke scenarios registered in
`tests/smoke/manifest.toml`:

- **Weekly wakeup.** A cron task with `next_cron_fire` in the past surfaces
  in `wg ready` after a coordinator tick (weekly gate + wakeup).
  Pinned by `cron_weekly_wakeup_becomes_ready.sh` (tests 1–2).
- **Missed-trigger catch-up.** A fire missed while the daemon was stopped
  (`next_cron_fire` already in the past at restart) is caught up on the
  first tick after restart — fired late, not silently dropped.
  `is_cron_due` returns true whenever `next_cron_fire <= now` regardless
  of how long ago the fire was. Pinned by
  `cron_weekly_wakeup_becomes_ready.sh` (test 3) and the
  `missed_cron_fire_is_caught_up_when_next_fire_is_past` unit test.
  **Catch-up is one-shot per window:** N missed weekly fires merge into a
  single catch-up fire (the schedule resumes from `now`, it does not
  backfill each missed slot). This is the durable default; a backfill mode
  for higher-cadence crons is a future design decision, not current
  behaviour.
- **Duplicate prevention / idempotency.** A Done cron task is reset to
  Open by Phase 2.95 exactly once per window: `reset_cron_task` only acts
  on `status == Done`, and after reset `next_cron_fire` is advanced to the
  NEXT schedule slot (future), so the same window is not re-fired on
  subsequent ticks. Pinned by `cron_recurring_no_duplicate_fire.sh`.
- **Paused / waiting gates hold.** A due cron task that is `paused` or in
  `Waiting` status is NOT ready — recurring triggers do not bypass
  task-state gates. Pinned by `cron_paused_state_skips_dispatch.sh`.

## Future direction (NOT current behaviour)

A TZ-aware cron mode (store an explicit IANA zone, fire at a fixed LOCAL
wall-clock that does not shift across DST) is a **deliberate future
schema addition**, gated by:

- a new `Task.cron_timezone: Option<String>` field,
- threading a `chrono_tz::Tz` through `parse_cron_expression` /
  `calculate_next_fire*` / `is_cron_due`,
- a `--cron-tz` flag on `wg add`,
- **updating or replacing** the `cron_evaluates_in_utc_no_local_dst_shift`
  pin so the new behaviour is intentional.

Until that lands, UTC-only is the contract. Do not add ad-hoc local-time
comparisons anywhere in the cron path — they will diverge from the pinned
UTC behaviour and silently mis-fire weekly jobs.
