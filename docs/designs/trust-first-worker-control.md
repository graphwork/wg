# Trust-first local worker control

## Regression reconstruction

The historical local worker launch immediately before commit `5b0e67b4`
(`Broker worker control-plane access over scoped IPC`) exported `WG_DIR`,
`WG_TASK_ID`, and `WG_AGENT_ID` to a normal `wg` CLI process. Consequently a
worker could use the same graph commands as an operator, and command code
already recorded ambient worker identity where applicable. The broker commit
then made `WG_WORKER_CAPABILITY` a hard CLI mode switch and reduced the command
set to own-task typed operations (`src/worker_cli.rs`). In particular,
`task_matches` rejected every unequal task ID with
`worker_control.cross_task_refused`, while the shipped worker prompt continued
to prescribe `wg add`, sibling edits, graph decomposition, and quality-pass
metadata changes.

Reproduction evidence is retained in two executable fixtures:

- `tests/smoke/scenarios/worker_control_capability_broker.sh` explicitly sets
  `[worker_control] mode = "scoped"` and proves the pre-fix
  `worker_control.cross_task_refused` behavior remains available by choice.
- `tests/smoke/scenarios/trust_first_local_worker_coordination.sh` omits policy
  (the historical compatibility case) and performs the old legitimate flow
  through public commands: downstream show/edit, subtask add/link/assignment,
  reprioritization, publish, and cross-task messaging.

To inspect the source-level historical fixture:

```bash
git show 5b0e67b4^:src/commands/spawn/execution.rs \
  | rg -n 'WG_DIR|WG_TASK_ID|WG_AGENT_ID'
git show --stat 5b0e67b4
```

## Policy

`WorkerControlMode` has three visible values:

| Mode | Intended actor | Graph authority |
|---|---|---|
| `trusted` (default) | ordinary local task worker | normal public local graph coordination |
| `scoped` | explicit strict project/task policy; evaluator/reviewer/assigner observation lanes; remote provider floor | own-task typed broker operations |
| `read-only` | explicit observation policy | reads/runtime bookkeeping only; graph mutations refused |

Project policy is `[worker_control] mode = "…"`. A task override is visible in
ordinary task metadata as `worker-control:trusted|scoped|read-only`. Structural
floors win over a widening task tag. `.quality-pass-*` is intentionally not an
observation lane: its stated purpose is cross-task metadata coordination.

`wg capabilities`, startup prompt context, spawn diagnostics/metadata, `wg
show`, `wg status`, service status, and the TUI inspector expose the effective
mode and restrictions before an instruction is attempted.

## Integrity retained

Trusted does not mean unfenced or unaudited:

1. The daemon validates graph identity plus the exact source task, generation,
   attempt ID, fence, lease epoch, and owner before every delegated command.
2. Each request is intent-journaled and append-only audited with exact
   `agent_id`, `attempt_id`, `fence`, mode, operation CID, and outcome.
3. Own-task completion stays on typed `completion-object` → immutable manifest
   → `submit` → exact review receipts → `land` (Land only) → derived `done`.
   Generic graph delegation cannot invoke service/admin, federation/review
   authority, or replace immutable completion evidence.
4. Existing command locks/CAS/transactions remain the mutation mechanism; the
   trusted lane shells back through the normal public CLI rather than writing
   graph bytes.
5. Revoked, released, or stale attempts fail validation before execution. The
   stale-fence unit regression remains in `src/commands/service/ipc.rs`.

These are consistency and evidence boundaries. Unequal source/target task IDs
are not themselves a security decision for a trusted local actor.

## Quality-pass recovery

Quality passes are advisory unless tagged `quality-pass:required`. A failed
optional `.quality-pass-*` with typed provider/local-infrastructure evidence
(`ExecutorConfig`, rate limit, transient 5xx, timeout, disk/wrapper failure, or
a normalized provider signal) yields an `AdvisoryQualityBypass`. Downstream
readiness therefore releases the unchanged batch and `wg show` carries the
loud reason. The dispatcher records the warning once when it observes the
release. A required tag preserves ordinary required-success failure semantics.

`tests/smoke/scenarios/quality_pass_advisory_provider_failure.sh` uses an
isolated fake Pi/OpenRouter 402 envelope to prove both paths credential-free.

## Prompt compatibility audit

Shipped cross-task instructions occur in:

- universal quality-pass guidance (`src/text/agent_guide.md`);
- decomposition/follow-up/prerequisite guidance in the worker prompt
  (`src/commands/spawn/context.rs` and `src/service/executor.rs`);
- the quality-pass template/design (`docs/designs/quality-pass.md`);
- Pi tools (`worksgood-pi/src/tools.ts`).

All ordinary local task forms resolve to `trusted`. Evaluator/reviewer/assigner
prompts do not receive cross-task mutation authority; they remain scoped and
communicate through their typed evidence/lifecycle lanes. Remote WG-Exec
providers remain independently UCAN/lease scoped and never enter this local
trusted delegation path.
