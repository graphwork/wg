# Pi evaluation plane rollout runbook

This runbook rolls the attempt-bound evaluation plane from **disabled** to
**bounded advisory**. It cannot enable a global hard gate. Routine FLIP remains
disabled; deep-readonly FLIP is an explicit, selective action.

The authoritative audit is:

```text
.wg/agency/evaluation-plane/canary-evidence.json
```

`wg evaluate rollout status --json` renders the same state. Config edits and
daemon reloads fail closed when `[evaluation].rollout_stage` differs from this
record.

## 1. Start disabled

```bash
wg evaluate rollout start
wg evaluate rollout status --json
```

Required output: `stage=disabled`, `auto_evaluate=false`,
`eval_gate_all=false`, `global_flip_enabled=false`. Starting twice at disabled
is idempotent. Do not edit the stage in TOML.

## 2. Credential-free lifecycle gate

Run the candidate binary through all owned scenarios:

```bash
WG_SMOKE_CANDIDATE_BIN="$PWD/target/debug/wg" \
  bash tests/smoke/scenarios/lazy_evaluation_tui_evidence.sh
WG_SMOKE_CANDIDATE_BIN="$PWD/target/debug/wg" \
  bash tests/smoke/scenarios/dedicated_pi_bounded_evaluation_lane.sh
WG_SMOKE_CANDIDATE_BIN="$PWD/target/debug/wg" \
  bash tests/smoke/scenarios/admission_deferral_backpressure.sh
WG_SMOKE_CANDIDATE_BIN="$PWD/target/debug/wg" \
  bash tests/smoke/scenarios/deep_readonly_flip_human_flow.sh
```

The evidence JSON for `fake-pi-lifecycle` must report zero never-ran
evaluations, stuck pending states, duplicate records/verdicts, evaluation worker
or build slots, and evaluation worktrees. It must report neutral admission
deferral and retained native Codex routing. Record before/after `wg viz --all
--no-tui` digests.

```bash
wg evaluate rollout advance \
  --stage fake-pi-validated --evidence fake-pi-lifecycle.json
```

Restart the daemon and re-run `status`; counts and evidence IDs must not change.

## 3. Live low-risk Pi/Luna bounded canary

Only continue if `pi --list-models` shows the exact configured route and its
login works. A missing route/credential is a loud stop, not Codex/Claude
fallback.

1. Complete one low-risk source normally and confirm an immutable candidate.
2. At `fake-pi-validated`, run the explicit pre-enable lane:

   ```bash
   wg evaluate run SOURCE --bounded --json > bounded-record.json
   ```

3. Confirm product `bounded`, state `consumed`, exact `pi:` route, one attempt,
   one verdict, Pi usage, no `.evaluate-*` task/agent/worktree, and unchanged
   terminal source status.
4. Create `bounded-live-canary` evidence and advance:

   ```bash
   wg evaluate rollout advance \
     --stage bounded-canary-passed --evidence bounded-live-canary.json
   ```

The 2026-07-28 canary used `pi:openai-codex:gpt-5.6-luna`; its evidence is in
`docs/reports/pi-evaluation-canary-evidence.json`.

## 4. One explicit deep-readonly FLIP canary

Do **not** call a shallow bounded grader FLIP. Only after the bounded canary:

```bash
wg evaluate run SOURCE --flip --json > deep-record.json
```

Accept the canary only when the report:

- has a `latent-intent` finding and a nonempty latent probe code;
- contains genuine counterfactual probe codes and cross-component evidence;
- observed all eight evidence classes and at least two repository files;
- has an audit containing only `deep_read_evidence`,
  `deep_search_repository`, `deep_read_repository`, and optionally the declared
  validation tool;
- leaves source/config/repository bytes unchanged (evaluation projections and
  immutable evidence are the only permitted writes); and
- uses the same exact Pi route on an explicit retry. Infrastructure/schema
  failure is visible and cannot change the source.

Then:

```bash
wg evaluate rollout advance \
  --stage deep-readonly-canary-passed --evidence deep-readonly-flip.json
```

The live Luna canary first failed closed on an invalid locator, then succeeded
on an explicit same-candidate/same-route retry after the locator grammar was
made unambiguous. Both attempts remain audited.

## 5. Enable bounded advisory only

```bash
wg evaluate rollout advance --stage advisory
wg evaluate rollout status --json
```

This is the only automatic mode enabled by this rollout:

```text
auto_evaluate=true
mode=bounded-advisory
eval_gate_all=false
global_flip_enabled=false
```

An inherited historical threshold is inert while `managed_rollout=true`;
`LazyEvaluationSelection` structurally refuses required applicability. Config
attempts to set `eval_gate_all=true`, enable global FLIP, change stage, or enable
auto-evaluation before canaries are rejected.

## 6. Observe and record

Observe at least two real source completions. Each must have exactly one
candidate-bound record and verdict, no stuck pending state, and no evaluation
worker/build/worktree occupancy. Record before/after Viz digests and thresholds:

```bash
wg evaluate rollout record-observation \
  --evidence source-observation.json
```

Rollback immediately on any of:

- duplicate semantic record or verdict;
- evaluation for a source that never ran;
- pending evaluation that remains stuck after its runner is terminal;
- evaluation consuming worker/build/worktree capacity;
- source status being reopened or changed by evaluation infrastructure failure;
- route drift, native Codex rewrite, or cross-executor fallback;
- global FLIP or any hard-gate applicability;
- non-idempotent daemon restart.

## 7. Roll back

Use the terminal controller, never hand-edit config:

```bash
wg evaluate rollout rollback \
  --reason 'duplicate verdict threshold reached'
wg service restart
wg evaluate rollout status --json
```

Rollback atomically returns the managed policy to disabled, preserves canary
and observation history, appends the operator reason, and keeps hard gates and
global FLIP off. It does not delete immutable evidence or alter source tasks.
