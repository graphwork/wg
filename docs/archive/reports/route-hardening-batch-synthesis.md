# Route-hardening batch synthesis (`synthesize-the-route`)

**Date:** 2026-09-16 · **Batch tasks:** `smoke-pin-completion`, `migrate-config-hygiene`,
`hermetic-test-triage`, `prune-stale-worktrees`, `opaque-pi-execution` — all five **done**.

## Verification performed (concrete commands + output)

1. **`cargo test --lib`** — `test result: ok. 3269 passed; 0 failed; 37 ignored; 0 measured;
   0 filtered out; finished in 25.16s`. **Green with ZERO failures** — the four
   pre-existing environment-dependent failures (`disk_sentinel`, `profile::named` ×3,
   `migrate_project_local_pi::rollback_restores_exact_bytes_cas`) no longer reproduce;
   `hermetic-test-triage` (commit `a5ee70ce`) made the suite hermetic against
   cross-test env races (`HOME`, `WG_GLOBAL_DIR`, provider-key env, PATH mutation) and
   the fix holds under this run.
2. **Three new smoke scenarios PASS.** Direct `bash tests/smoke/scenarios/<name>.sh`
   is correctly refused by `_helpers.sh` (the fail-closed subreaper guard), so each
   scenario was run through the sanctioned bridge
   `WG_SMOKE_DIRECT_SCENARIO=… cargo test --lib run_explicit_scenario_under_process_harness -- --ignored --nocapture`:
   * `completion_empty_diff_idempotence` — **ok** (101.48s)
   * `fresh_route_receipts_probe` — **ok** (94.55s)
   * `completion_legacy_environment_drift_renewal` — **ok** (94.90s)
   All three are credential-free (deterministic reviewer stubs / fake-pi fixtures; no live
   model call — verified by grepping the scenario sources for the fake-pi shim patterns).
3. **Config lint + routes.** `wg config lint` is **admin-refused for trusted workers**
   (`worker_control.admin_operation_refused`, `src/worker_cli.rs` refuses the whole
   `Commands::Config` / `Commands::Migrate` families — same boundary the opaque trial
   documented). Manual lint-equivalent replication over both config surfaces found **no
   deprecation debt**: no bare provider-prefix specs, no `executor` / `api_key_env` keys,
   `dispatcher.poll_interval` → `safety_interval` rename already applied, and `wg status`
   (which loads the config and prints one-shot deprecation warnings) printed none.
   **Route identity (from `wg status`, byte-exact):**
   `default=pi:lunaroute/glm-5.3-flash [explicit: models.default.model]`,
   `strong=pi:lunaroute/glm-5.3-flash [explicit: tiers.standard]`,
   `weak=pi:lunaroute/deepseek-4.1-flash [explicit: tiers.fast]`.
   ⚠️ **Spelling note:** this task's check text names the *colon* form
   (`pi:lunaroute:glm-5.3-flash`); the batch **deliberately migrated** the project routes
   to the *slash* form (`pi:lunaroute/glm-5.3-flash`) during `opaque-pi-execution`
   (commit `08454814` + `wg migrate opaque-pi-route`), because the real `pi` runtime
   rejects the colon form outright. The route *identity* (glm-5.3-flash strong /
   deepseek-4.1-flash weak, same per-role reasoning) is unchanged; only the executable
   spelling moved. The colon-form expectation in the check text predates that migration
   and is superseded by it.
4. **`wg status` / daemon health.** `Service: running (PID 1846632, …)`,
   `Dispatcher: handler=pi model=pi:lunaroute/glm-5.3-flash`. The daemon's
   `/proc/<pid>/environ` contains **no** `WG_EXPERIMENTAL_OPAQUE_ASSIGNMENT` (probe match
   exit 1) — the daemon is on the **DEFAULT (non-experimental)** configuration, as the
   opaque trial's post-trial restore requires. **Disk section pruned:**
   `Healthy — 115.2 GiB projected headroom, 1 active target(s), 6 stale;
   worktrees=555 MiB, .wg/agents=326 MiB, .wg/log=1 MiB`. Per `prune-stale-worktrees`
   logs: worktrees **6839→123 MiB** (57 stale worktrees removed), dead-agent state
   **1479→277 MiB** (78 entries purged, 31 orphaned smoke process groups killed),
   **26.2 GB** of stale build caches reaped. The current 555/326 MiB figures are the
   post-prune steady state regrown by this batch's own attempts; the residual
   **6 stale targets are WG-guard-protected** (555 `.wg-owned-baseline` dirs) and were
   deliberately skipped — not a prune failure, but see residual notes.
5. **Opaque trial report** — `docs/reports/opaque-trial-lunaroute.md` exists (plus
   `opaque-trial-lunaroute-probe-evidence.txt`) and its final verdict is **coherent with
   the evidence**: first attempt honestly recorded two blockers (admin-refused daemon
   restart; capability-probe row-count ambiguity from `-background` catalog siblings);
   the operator-run completion records the probe exact-match fix (`222154d5`), the
   slash-route migration + validator acceptance (`08454814`), live end-to-end admission
   of both routes with byte-exact `execution.opaque_route`, and a **"ready for wider
   opt-in, not yet default"** recommendation — correctly withheld from default promotion
   because the sibling-ambiguous classification has no end-to-end smoke pin yet.

## What changed this batch

* **Stable environment identity for completion evidence** (`a2686bad`, plus `c3964da1`
  excluding the WG build-isolation overlay from the identity): worker-captured
  deterministic-validation evidence now carries a stable environment identity and
  re-validates across the worker→operator process boundary without spurious renewal.
* **Legacy renewal at Done** (`68d1228a`): evidence captured in the historical v1 shape
  (no `environment_stable_identity`) fails strict re-validation with an
  environment-drift classification and completes through the sanctioned Done-step
  renewal (`src/commands/completion_done.rs renew_environment_drifted_validation`),
  re-running the configured/baseline commands at the integrated commit and recording
  fresh captures.
* **Reviewer empty-diff idempotence + deliverable-presence facts** (`2901fc13`,
  `c14b5272`, `17de47fa`): a Land task whose deliverable already exists in the base
  (empty candidate diff) now reaches FLIP pass + Eval pass and exact strict acceptance;
  the FLIP phase-II material carries controller-computed `deliverable_presence` facts
  and the EMPTY-DIFF IDEMPOTENCE rule while phase I stays blind to them.
* **TUI route visibility + sticky pin** (`27332ef7`): the primary HUD detail builder
  now renders the full route story (previously only the drill-down builder did, as
  unlabeled lines) via a shared `append_route_sections`, and the inspector pin is
  sticky across refresh.
* **Hygiene:** `hermetic-test-triage` (lib suite hermetic, `a5ee70ce`);
  `prune-stale-worktrees` (~6.8 GiB worktrees + ~1.4 GiB dead-agent state + 26.2 GB
  caches reclaimed); `opaque-pi-execution` (probe classification fix + slash-route
  first-class acceptance + migration); `60de44a8` reaps orphaned descendant process
  groups on agent death (spawn pgid registry), closing the smoke-fixture leak class.

## What the smoke scenarios now pin

* **`completion_empty_diff_idempotence`** — empty-diff Land tasks cannot be rejected for
  "nothing delivered"; the phase-II reviewer material provably carries
  `deliverable_presence: present` and the idempotence rule; no validation renewal fires.
* **`fresh_route_receipts_probe`** — a task dispatched on the project routes completes
  with every route receipt byte-exact: unsplit strong-route launch argv, weak-route
  review invocations, exact source-provider launch binding, agent-registry attribution,
  `actual_model`/`actual_executor`, and immutable strict-approved FLIP+Eval receipts
  carrying the exact routes; the Pi capability probe is exercised for both lanes.
* **`completion_legacy_environment_drift_renewal`** — the positive control (stable
  identity re-validates without renewal) and the legacy-v1 downgrade path (strict
  re-validation fails, Done-step renewal produces fresh captures + stable identity,
  original evidence re-verified under TolerantDrift).

## Opaque-trial verdict

**Ready for a wider opt-in period on this project's real routes; not yet for default
promotion.** The one named residual gap: the sibling-ambiguous capability
classification (`222154d5`) is pinned only by six `execution_assignment` unit tests —
`pi_opaque_execution_assignment.sh`'s fake-pi fixture answers `--list-models` with a
single exact row and never exercises exact-match-over-fuzzy-siblings end-to-end. An
opaque-path smoke case with an exact row plus a `-background`-style sibling should be
added before default promotion.

## Non-convergences (flagged, not papered over)

1. **`migrate-config-hygiene` did NOT execute its migrations.** The task is marked done,
   but its worker log records that `wg config lint`, `wg migrate config --dry-run`,
   `wg config --models` and `wg service status` were **all**
   `worker_control.admin_operation_refused`, and it escalated via request-help — no
   `wg migrate config --all` and **no `wg migrate project-local-pi
   --cleanup-global-routing` ever ran**. The stale machine-global routing is still on
   disk: `~/.wg/config.toml` still carries `[agent].model = "pi:zai:glm-5.2"`,
   `[tiers]` and the full `[models.*]` zai tables, and `~/.wg/active-profile` still
   exists (`alpha`, mtime today 17:50). **Impact:** low — project resolution is
   project-local (`worksgood.toml` wins; `active-profile` is ignored; `wg status`
   confirms routes sourced from the project file), so this is dead weight, not a
   routing risk. **Action needed:** an operator must run
   `wg migrate config --all` + `wg migrate project-local-pi --cleanup-global-routing`
   (or grant a config-admin lane); a follow-up task is warranted.
2. **`wg config lint` remains unverifiable by trusted workers** (family-level refusal,
   by design). The manual replication above is strong but not the authoritative lint
   run; the authoritative run is operator-only.
3. **Route-spelling drift vs the batch check text:** the check's literal colon-form
   expectation is superseded by the deliberate slash migration (see check 3). Anyone
   re-running the original check verbatim will see a false mismatch.
4. **Disk:** 6 stale build targets remain, protected by the WG guard
   (`.wg-owned-baseline` dirs) — "stale near zero" was met for worktrees/agents but not
   literally for targets. Likely correct to leave (guard semantics), but it is not
   zero.

## Residual risks

* **Weak-tier FLIP/Eval calibration on real tasks — watch closely.** This batch's own
  review lane (deepseek-4.1-flash) produced several `FlipRejected` / `EvalRejected`
  outcomes: `smoke-pin-completion`'s first candidate was semantically rejected
  (`completion.missing_authoritative_runtime_evidence` — a legitimate catch, the
  manifest genuinely lacked runtime evidence for the scenarios), and several other
  batch tasks show FlipRejected lanes before landing. The strictness is currently
  *correct* (it forced real evidence), but the same model also drives `.flip`/`.assign`
  one-shots; if it starts rejecting sound candidates on style rather than substance,
  calibration (or a stronger weak tier) will be needed. Recommend sampling a few
  rejected verdicts against their candidates next batch.
* **Weak-tier cost accounting reads $0.000000** on every lunaroute receipt despite real
  token usage (in/out counted, cost zero) — the model-registry rate table has no
  lunaroute entries and pi's own `usage.cost.total` is zero for this provider, so
  `wg spend`/`wg stats` under-report this plane. Cosmetic but worth a rates entry.
* **Opaque-path promotion gap** (above): sibling-ambiguous probe classification lacks
  an end-to-end smoke pin.
* **Stale global routing debt** (above): harmless today, but any future change that
  re-weights global-vs-project resolution precedence would resurface it loudly.

## Bottom line

The batch converged on its code and test goals: the lib suite is fully green and now
hermetic, the three completion-pipeline fixes are smoke-pinned end-to-end, the disk
reclaim landed, and the opaque trial completed under operator authority with a
well-evidenced opt-in verdict. The one genuinely unfinished item is the config-hygiene
migration itself, which requires operator (admin) authority and was closed without
being performed — it should be re-run by an operator, not silently accepted as done.
