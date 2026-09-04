# Maze-free service and completion recovery

This is the supported operator and contributor flow when a daemon is restarted while a
reviewed Land candidate is waiting to publish. It composes service readiness with
completion reconciliation without weakening immutable receipt or target-fence checks.

## Operator flow

### 1. Restart the service

```sh
wg service stop --force
wg service start --force
```

`wg service start` returns success only after the replacement daemon answers the
instance-nonce/PID readiness challenge. A timeout exits nonzero, prints
`WG SERVICE START FAILED`, includes a bounded daemon-log tail, and names the retry:

```sh
wg service start --force
```

Do not infer readiness from a stale PID or state file. `wg service status` is a useful
follow-up observation, but the successful start itself is readiness-confirmed.

### 2. Inspect a retained landing candidate

A reviewed candidate can park as `Waiting/LandingPending` when the attached integration
checkout contains user changes or its target advances. The source worker is released;
there is nothing to repair or resubmit from that worker.

```sh
wg show <task>
wg merge-resolution status <task>
```

The human status includes the immutable candidate digest, exact task/generation/attempt/
fence binding, reconciliation state and receipt, whether the source worker was released,
finalizer recovery authority, and one supported next action.

Preserve user changes by committing them elsewhere or otherwise moving them out of the
attached integration checkout, then make that checkout clean. WG-created registered
`.wg-worktrees/<agent>/` paths are excluded through repository-local Git administrative
state; WG does not edit the project's `.gitignore` and does not hide arbitrary siblings.

### 3. Resume finalization

```sh
wg resume <task> --only
```

For a descendant-only target advance, the finalizer deterministically integrates the
retained candidate, reruns configured validation plus the baseline validation, writes a
new immutable target-binding receipt, rechecks the exact ref fence, and lands. The
command is idempotent with coordinator recovery and crash replay. It does not rerun the
source worker or semantic review.

Do **not** use `wg retry`, `wg requeue`, or `wg unclaim` for a retained landing candidate.
Do not run `git reset --hard`, rebase the candidate, cherry-pick it, or delete a
WG-managed worktree. Those operations either target the wrong lifecycle transition or
break the evidence binding that protects publication.

Divergence, merge conflict, changed requirements/dependency outputs, failed refreshed
validation, user dirtiness, or a stale fence remain fail-closed. Candidate and user bytes
remain intact. Correct a transient named condition and repeat `wg resume`; if the
candidate cannot reconcile, use `wg reset <task>` only when `wg show` or
`wg merge-resolution status` names it as the authorized new-generation fallback.

## Why the fence remains strict

The original review receipts bind the selected candidate and requirements. A moving
landing target changes the combined tree, so ancestry alone is not publication authority.
Reconciliation therefore binds all of the following before landing:

- task, generation, attempt, and fence;
- immutable manifest and candidate commit;
- expected and observed target OIDs;
- requirements and dependency-output inputs;
- configured and baseline validation evidence;
- deterministic integration commit and exact publication ref.

Any mismatch parks or blocks the task rather than silently accepting, mutating, or
discarding content.

## Contributor regression flow

Two grow-only smoke entries pin the real terminal behavior and share one candidate binary:

```sh
WG_SMOKE_CANDIDATE_BIN=/path/to/wg \
  bash tests/smoke/scenarios/service_start_readiness_pty.sh
WG_SMOKE_CANDIDATE_BIN=/path/to/wg \
  bash tests/smoke/scenarios/completion_landing_reconciliation.sh
```

The first repeatedly challenges immediate PTY stop/start success and the loud timeout
path. The second composes a readiness-confirmed PTY restart with a released-worker
`LandingPending` candidate, a WG-managed worktree, user dirtiness, a descendant target
advance, renewed evidence, and final landing. It asserts that candidate, target, and user
bytes survive and that no source/reviewer rerun or manual history rewrite occurs.
