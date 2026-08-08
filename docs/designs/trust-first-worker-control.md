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

Reproduction evidence is retained in executable before/after fixtures:

- `tests/historical/trust_first_worker_control_regression.sh` builds the pinned
  pre-fix tree at `da286458ac640a6c4a49b269284c39e1d9ff3fdf`, then runs that
  tree's own real-daemon/Fake-Pi broker smoke to reproduce the actual scoped
  graph/cross-task refusal. It next runs the current candidate's trust-first
  quality-pass flow. This is runtime evidence, not a `git show` inspection.
- `tests/smoke/scenarios/worker_control_capability_broker.sh` explicitly sets
  `[worker_control] mode = "scoped"` and proves that strict behavior remains
  available by current policy choice.
- `tests/smoke/scenarios/trust_first_local_worker_coordination.sh` omits policy
  and performs the legitimate flow through public commands: downstream
  show/edit, subtask add/link/assignment, reprioritization, publish, messaging,
  immutable completion, and downstream release.

## Policy

`WorkerControlMode` has three visible values:

| Mode | Intended actor | Graph authority |
|---|---|---|
| `trusted` (default) | ordinary local task worker | normal public local graph coordination |
| `scoped` | explicit strict project/task policy; evaluator/reviewer/assigner observation lanes; remote provider floor | own-task typed broker operations |
| `read-only` | explicit observation policy | reads/runtime bookkeeping only; graph mutations refused |

Project policy is `[worker_control] mode = "…"`. A task override is visible in
ordinary task metadata as `worker-control:trusted|scoped|read-only`. Structural
floors win over a widening task tag. Sandboxed inbound tasks carry the visible
`worker-control:inbound` (legacy-compatible aliases: `content:inbound` and
`sandboxed-inbound`) floor and are always `read-only`, even if attacker-controlled
metadata also asks for `worker-control:trusted`. `.quality-pass-*` is intentionally
not an observation lane: its stated purpose is cross-task metadata coordination.

`wg capabilities`, startup prompt context, spawn diagnostics/metadata, `wg
show`, `wg status`, service status, and the TUI inspector expose the effective
mode and restrictions before an instruction is attempted.

## Integrity retained

Trusted does not mean unfenced or unaudited:

1. Trusted coordination runs the normal CLI directly against the canonical
   graph; it is not re-executed as a daemon `GraphCli` broker operation.
2. At the graph commit boundary, under the graph lock, WG revalidates graph
   identity plus the exact source task, generation, attempt ID, fence, lease
   epoch, owner, and opaque capability. Revoked/released/stale attempts fail
   before replacement.
3. Every changed task receives a `trusted-graph-mutation` lifecycle event and
   task-log attribution naming command, source, generation, actor, attempt,
   fence, and lease. The lifecycle ledger is fsynced before graph replacement;
   a second fsynced append-only `trusted-mutation-audit.jsonl` records the exact
   committed task IDs.
4. Own-task completion stays on typed `completion-object` → immutable manifest
   → `submit` → exact review receipts → `land` (Land only) → derived `done`.
   The direct CLI boundary positively names ordinary coordination verbs; omitted
   families (`trace`, `func`, `replay`, service/admin, federation/review) fail
   closed and cannot replace immutable completion evidence.
5. Existing command locks/CAS/transactions remain the mutation mechanism; no
   alternate graph writer or permission ceremony is introduced.

These are consistency and evidence boundaries. Unequal source/target task IDs
are not themselves a security decision for a trusted local actor.

## Quality-pass recovery

Quality passes are advisory unless tagged `quality-pass:required`. A failed
optional `.quality-pass-*` with typed provider/local-infrastructure evidence
(`ExecutorConfig`, rate limit, transient 5xx, timeout, disk/wrapper failure, or
an explicitly allowlisted normalized provider signal with route provenance)
may yield an `AdvisoryQualityBypass`. Before admission WG create-once snapshots
the exact transitive downstream batch for that task generation. Release occurs
only when current task IDs and serialized metadata match that baseline; a
quality worker that changed the batch before failing remains an ordinary
blocker. The satisfaction boundary itself emits the loud warning, so readiness,
show/completion, and dispatcher paths cannot silently consume the bypass; the
dispatcher additionally persists the warning once. Authentication failures are
never advisory infrastructure. A required tag preserves ordinary
required-success failure semantics.

`tests/smoke/scenarios/quality_pass_advisory_provider_failure.sh` uses an
isolated fake Pi/OpenRouter 402 envelope to prove both paths credential-free.

## Prompt compatibility audit

The executable inventory in `tests/prompt_snapshots.rs` scans every shipped
worker/system prompt surface: the universal guide; spawn context and executor;
coordinator, assignment, triage, and human-dispatch prompt builders; the review
prompt; and both Pi tool/backend surfaces. Every discovered cross-task command
is checked against compatible trusted authority. The quality-pass design/template
is checked alongside that shipped inventory.

All ordinary local task forms resolve to `trusted`. Evaluator/reviewer/assigner
prompts do not receive cross-task mutation authority; they remain scoped and
communicate through their typed evidence/lifecycle lanes. Remote WG-Exec
providers remain independently UCAN/lease scoped and never enter this local
trusted direct-CLI path.
