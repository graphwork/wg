# Completion repair and help

Semantic-rejection help only projects a verified current FLIP/Eval receipt onto the existing completion-repair attention state. When a strict review's findings are exclusively the reserved `completion.missing_authoritative_runtime_evidence` class and identify one exact existing runnable check, WG enters that bounded help state automatically; other semantic rejections still require candidate repair or an explicit help request. It does not create scheduler hierarchy, fabricate deterministic evidence, accept a candidate, retry a provider, rerun source/reviewer work, widen scope, or lower a gate.

`wg done TASK` has four normal outcomes:

1. **Accepted** — WG runs the exact deterministic contract, captures host-bound immutable evidence for the current source attempt/candidate, obtains the configured semantic receipts, publishes under the existing lease/fence rules, and derives `Done` exactly once.
2. **Repair in the same worker** — a known deterministic command failure keeps the task, source attempt, session, worktree, candidate authority, and saved work in place. The command returns a structured failure containing the command/exit category, a bounded redacted diagnostic excerpt (explicitly untrusted), evidence CID, candidate identity, validation identity, finite budget, and one next action. Repair meaningful bytes and rerun the unchanged `wg done` command. No reviewer is called for a known command failure.
3. **Recoverable semantic repair in the same worker** — a strict FLIP/Eval rejection whose bounded findings are a recoverable class records a `Repairing` disposition on the existing `CompletionRepairState` episode with a `recovery_round`, delivers a bounded corrective checkpoint, and writes a coordinator-auto-resumable `Timer` wait on the same generation, attempt, session, fence, and worktree. This path has **no acceptance path**: a recovery round never marks the task `Done`; the candidate must pass the unchanged reviewer. Gate-weakening, scope ambiguity, missing authority, and defect/incomplete-implementation classes are irrecoverable and go straight to `NeedsAttention`; a repeated rejection on unchanged bytes or an exhausted budget also escalates to `NeedsAttention`. See `docs/design-review-recovery.md` for the recoverable-versus-irrecoverable classification.
4. **NeedsAttention** — unchanged repeated bytes, the finite repair ceiling, an unknown result, missing authority, scope ambiguity, a requested contract correction, or an explicit request after a current receipt-verified semantic rejection stops automatic completion. `wg show`/`wg status` and the TUI name the underlying blocker, exact source/candidate/evidence binding, affected downstream tasks, saved-work location, and one safe operator action. Saved work and evidence stay retained.

The default deterministic repair budget is **two opportunities per task episode**. Restart, replay, duplicate feedback, output-only changes, and a new process do not replenish it. A changed source candidate may consume the next opportunity but cannot extend the episode indefinitely. Semantic rejection keeps its separate existing candidate-bound review budget and remains fail-closed. Automatic evidence-gap help and an explicit help request both record the current verified rejection receipt on the existing attention surface; neither accepts work, manufactures a deterministic failure, retries the provider, or calls the unchanged reviewer. Mixed findings, multiple proposed commands, empty commands, and missing implementation/tests are never auto-classified as contract-only repair.

## Preflight and evidence

Every worker prompt and `wg show TASK` expose the exact checks in enforced order, each check's provenance, and the evidence-capture mechanism. `wg contract TASK` prints the same plan without mutation. Commands in `Task.validation_commands` (and the historical singular `Task.verify`) are operator/repository-authorized hard gates. The built-in Land baseline, or the non-Land regular-file/immutable-artifact check, is also shown and cannot be removed. `## Validation` prose remains acceptance criteria, not executable authority; WG does not guess shell commands from prose.

Validation evidence is authoritative only when `wg done` captured and registered it against the exact task requirements, generation, attempt, fence, repository/worktree, command and execution-environment identity, and source revision. Workers may add repeatable `--check '<COMMAND>'` arguments to `wg done`; these use the same trustworthy capture but are labeled optional review evidence and never become or waive a hard gate. A report may mention earlier implementation commits, but only host-bound evidence for the selected candidate counts. Review/publication/reload receipts created later are postconditions and cannot be cited as candidate inputs.

## Permitted repair boundary

Default: `task-and-validation-fixtures`. This is an explicit worker authority policy and audited operator boundary, not a guessed filename allowlist: WG can cryptographically prevent gate/requirements/fence changes, but it cannot infer from a path alone whether production code is relevant to the task. Workers must request approval when relevance is ambiguous; completion evidence never expands the boundary.

- Allowed without another decision: task implementation plus tests/fixtures directly exercised by the unchanged required checks.
- Requires explicit operator approval: unrelated production behavior, arbitrary repository cleanup, or any requested expansion beyond that boundary.
- Never allowed as “repair”: weakening/removing a required gate, bypassing candidate/session/fence proof, or treating diagnostic output as authority.

An operator may choose a narrower `task-only` boundary or explicitly approve `repository` repair:

```text
wg contract TASK --repair-boundary task-only
wg contract TASK --repair-boundary task-and-validation-fixtures
wg contract TASK --repair-boundary repository
```

A worker requests exactly one decision without terminally failing the task:

```text
wg fail TASK --intent request-help --reason '<specific scope decision>'
wg fail TASK --intent request-contract-correction --reason '<specific missing-check proposal>'
```

These intents are also available after the current candidate's exact FLIP or Eval semantic rejection. Superseded/forged receipts and wrong task, attempt, fence, requirements, or candidate bindings are rejected. Replaying the same request, restarting the daemon, or observing the worker exit retains one attention event; a later timeout remains separate failure classification and cannot hide that event. The retained worktree is stored as a display-only locator in the attention projection so both `wg show` and `wg status` keep it visible across restart; it grants no filesystem or completion authority. `deliberate-stop` needs no failed test and remains terminal unless an operator explicitly starts a new attempt.

The proposal is untrusted text and cannot itself change authority. An attended operator adds one approved check while preserving all existing checks:

```text
wg contract TASK --add-validation-command '<exact command>'
```

A policy/check update is allowed only before assignment or from an explicit in-progress NeedsAttention hold. It is refused for terminal/finalizer-waiting tasks, is logged, changes the live requirements digest, invalidates any stale selected candidate/review binding, and reopens the same authorized source episode. Old failure evidence retains its original requirements binding rather than being rewritten. An operator may explicitly adjust the total ceiling with `--deterministic-repair-budget N`. `wg fail TASK --intent deliberate-stop --reason '<why>'` preserves intentional cannot-complete behavior. Pause/cancel/abandonment, security/integrity failures, credentials, and unsafe/ambiguous session recovery remain authoritative and are not routed through completion repair or provider backoff.

## Visibility and notification

The graph projection is read-only and cycle-safe: cycles are traversed with deduplication and are not themselves considered stalls. A source actively repairing, or an ordinary valid wait, is not reported as a stalled chain. NeedsAttention records carry a deterministic event ID, so retries and daemon restarts do not spam or invoke models. WG uses the existing graph-change/TUI bridge. If the task has a typed `.chat-*`/`.user-*` origin parent, the same deduplicated event is also queued through the existing task-message bridge. Projects without such an origin or an external notifier retain the event in `show`/`status`/TUI; no new notification backend is invented.
