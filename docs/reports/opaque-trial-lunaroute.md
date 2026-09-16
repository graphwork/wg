# Opaque Pi execution trial — real lunaroute routes (`opaque-pi-execution`)

**Task:** `opaque-pi-execution` · **Date:** 2026-09-16 · **Design:** `docs/design-opaque-execution-assignment-experiment.md`
**Scope:** downstream real-provider acceptance trial of the immutable opaque execution assignment experiment against the project's real routes (`pi:lunaroute:glm-5.3-flash` task agent, `pi:lunaroute:deepseek-4.1-flash` evaluator/hermetic lane).

## Verdict up front

**The trial could not be completed end-to-end, and the evidence gathered so far is negative for the current route shape.** Two independent, evidence-backed blockers were found:

1. **The required experimental-daemon restart is admin-refused for trusted workers** — the trial never reached `WG_EXPERIMENTAL_OPAQUE_ASSIGNMENT=1` dispatch.
2. **The project routes cannot pass the experiment's capability probe as configured.** A byte-exact replication of the admission preflight against the real `pi` binary shows 0 resolved rows for the opaque colon-form route, runtime `--model` rejection of the same suffix, and a 2-row ambiguity for the slash-form migration target. Either way the probe's "exactly one Pi-resolved selection" rule fails deterministically.

**Recommendation: do not promote the experiment toward default on the current route configuration.** Details and the fix list below.

## Environment (verified before the trial)

- Default daemon healthy on the DEFAULT (non-experimental) configuration:
  `dispatcher handler=pi model=pi:lunaroute:glm-5.3-flash`, project routes resolved
  `default/strong = pi:lunaroute:glm-5.3-flash`, `weak = pi:lunaroute:deepseek-4.1-flash`
  (byte-exact values confirmed in `worksgood.toml`).
- The lunaroute provider is registered in two surfaces, exactly as the task states:
  - statically in `~/.pi/agent/models.json` (`providers.lunaroute` with both `glm-5.3-flash` and `deepseek-4.1-flash`, plus `-background` variants), and
  - via `@lunaroute/pi-extension` in `~/.pi/agent/settings.json` `packages`.
- `pi` 0.85.1 on PATH; WG pi plugin at `~/.cache/wg/worksgood-pi/0.3.0`.

## Step 1 — experimental daemon: **BLOCKED (admin boundary)**

The trusted-worker control boundary refuses all service administration. Attempting the
command family from the worker session returns, before any effect:

```
Error: worker_control.admin_operation_refused: command is outside trusted local graph coordination
```

- Verified live with `wg service status` from the worker session; `wg service stop` /
  `wg service start` are the same `Commands::Service` family and are refused identically
  (`src/worker_cli.rs` `maybe_run`, and the broker-side list in
  `src/commands/service/ipc.rs`).
- This is intentional, recently hardened worker-control policy ("service/admin … remain
  protected"), pinned by the `integration_service_control_permissions` test and described
  in `docs/reliable-main-baseline-diagnosis.md`.
- Therefore: no experimental daemon was started, no probe task was dispatched through the
  opaque path, and no bound `execution-assignment/assignment.json` evidence was produced.
  The default daemon was left untouched (and was healthy before, during, and after).
- No `WG-OPAQUE-*` refusals were observed — none could occur without the experimental
  daemon. The only refusal class observed was `worker_control.admin_operation_refused`.

**Consequence:** the end-to-end leg of the trial (steps 2–3 as written) requires an
operator-run daemon restart (`wg service stop` → `WG_EXPERIMENTAL_OPAQUE_ASSIGNMENT=1
wg service start` → later restore) or an explicit operator grant of service administration
to the trial lane.

## Step 2 — admission probe: **would FAIL for both lanes (replicated byte-exact)**

Although the daemon restart was blocked, the probe itself is fully replicable outside the
daemon: `execution_assignment::preflight()` runs the pinned executable with the lane's
`capability_argv` plus `--list-models <opaque_route>`, where `opaque_route` is the byte-exact
suffix after the single recognized `pi:` envelope (`parse_opaque_pi_route`). The probe is
ready only when exactly one data row returns; 0 rows (including pi's successful
`No models matching`) is deterministic missing capability.

Replication against the real `pi` binary and the real lunaroute registration
(`PI_CODING_AGENT_DIR` = `~/.pi/agent`, cwd = project root, `--offline`):

| Lane | Query (`--list-models`) | Result | Probe verdict |
|---|---|---|---|
| worker (`--offline -ne --no-skills --no-prompt-templates --no-context-files`) | `lunaroute:glm-5.3-flash` | `No models matching` (exit 0, **0 rows**) | MissingRequiredCapability (deterministic) |
| hermetic (`--offline -ne --no-tools … --no-session`) | `lunaroute:deepseek-4.1-flash` | `No models matching` (exit 0, **0 rows**) | MissingRequiredCapability (deterministic) |
| worker, post-`migrate` slash shape | `lunaroute/glm-5.3-flash` | **2 rows** (`glm-5.3-flash`, `glm-5.3-flash-background`) | ambiguous → MissingRequiredCapability |
| hermetic, post-`migrate` slash shape | `lunaroute/deepseek-4.1-flash` | **2 rows** (`deepseek-4.1-flash`, `deepseek-4.1-flash-background`) | ambiguous → MissingRequiredCapability |

Runtime resolution (what execution would pass as `--model`) confirms the colon form is
dead on pi's side regardless of the probe:

```
$ pi --offline -ne … --model 'lunaroute:glm-5.3-flash' --print
Error: Model "lunaroute:glm-5.3-flash" not found. Use --list-models to see available models.
$ pi --offline -ne … --model 'lunaroute/glm-5.3-flash' --print 'Reply with exactly: ok'
ok
$ pi --offline -ne --no-tools … --no-session --model 'lunaroute/deepseek-4.1-flash' --print 'Reply with exactly: ok'
ok
```

### Interpretation

- The experiment's admission would **defer without claiming** the probe task and persist
  backoff (missing-capability → 60 s cap) — the probe task would sit admission-blocked
  indefinitely, producing no `execution-assignment/assignment.json` evidence.
- The registered `-background` sibling variants in `models.json` make the slash-form
  fuzzy query **ambiguous** (2 rows), defeating the "exactly one row" rule even after a
  loss-aware `wg migrate opaque-pi-route` conversion. Notably the `-background` variants
  themselves resolve uniquely (`lunaroute/glm-5.3-flash-background` → 1 row) — but those
  are not the routes the project selects.
- The static models.json registration **does** make the models resolvable at runtime in
  slash form — the registration premise of the task is sound; only the probe's row-count
  rule and the route spelling stand between here and admission.

## Step 4 — recommendation

**Do not promote the opaque experiment toward default yet.** The trial surfaced three
specific, fixable blockers, in priority order:

1. **Trial authorization path.** A trusted worker cannot restart the daemon by design.
   Real-provider trials of this experiment need either an operator-run
   stop/start sequence around the trial window, or an explicit operator-granted
   admin lane for the trial task. (The concurrency note in the task was correct — a live
   worker, `reap-orphan-descendant-pgroups`, was running during this attempt; the restart
   was *not* performed.)
2. **Route spelling.** The project routes must migrate from the split colon form
   (`pi:lunaroute:glm-5.3-flash`) to the slash form (`pi:lunaroute/glm-5.3-flash`,
   `pi:lunaroute/deepseek-4.1-flash`) via `wg migrate opaque-pi-route` — pi's runtime
   accepts slash form (verified above) and rejects colon form outright.
3. **Probe ambiguity.** After migration the probe still fails on row count because of the
   `-background` sibling ids. Pick one:
   - WG probe: resolve readiness from pi's *exact/ranked top* match rather than raw row
     count (requires a pi-side exact-match query surface), or
   - registration: drop the `-background` variants from the static `models.json`
     registration used by the hermetic lane, or register distinct unambiguous ids the
     project routes point at.

Items 2–3 are small, mechanical, and testable with the same standalone probe replication
used here; item 1 is an operator decision. A retry after 1–3 should reproduce the full
end-to-end leg (experimental daemon → admission → probe task → byte-exact route in
`execution-assignment/assignment.json`).

## Evidence appendix

- Probe replication transcript (all commands + outputs): captured during this attempt;
  key rows reproduced in the table above.
- Code anchors: `src/execution_assignment.rs` `preflight()` (probe + row-count rule,
  `matches == 1`), `src/config.rs` `parse_opaque_pi_route` (single `pi:` envelope strip),
  `src/worker_cli.rs` `maybe_run` (trusted-worker service refusal).
- No files under `execution-assignment/` were created; no graph state was mutated by this
  trial beyond its own logs/artifacts; the default daemon was never stopped.
