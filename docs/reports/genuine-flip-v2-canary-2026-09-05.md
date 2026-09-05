# Installed v2 genuine-FLIP bootstrap canary — 2026-09-05

## Result

The installed CLI and live project daemon were byte-identical, post-`88e79dc9`
images before this canary was submitted. The source proof-binding matrix, blind
prompt visibility test, semantic-rejection/restart smoke, landing FIFO/restart
smoke, Rust policy checks, Pi-plugin tests, and embed-sync check passed. No
`cargo install` was run by this worker.

A preliminary submission of this report is used below to record concrete v2
receipt and phase-object CIDs. The final candidate necessarily has new receipt
CIDs because a receipt binds the candidate manifest and cannot be embedded in
the tree it hashes; the final current-candidate CIDs and post-reload projection
check are therefore also retained in the task's immutable completion objects and
lifecycle log.

## Installed image identity

Evidence captured before completion:

- source `HEAD`: `88e79dc94d8c89ab70d3c7407d36b47a013b8ea1`
  (`Bind FLIP review to immutable phase executions`);
- installed executable: `/home/bot/.cargo/bin/wg`;
- installed SHA-256:
  `d6d416c870e5ae7714b518cc56a7025b9359985fdc9eb5f58c0393b26674c806`;
- live daemon PID: `2203209`, started `2026-09-05 05:31:37 +0200`;
- `/proc/2203209/exe` and `/home/bot/.cargo/bin/wg` both had inode `4723454`,
  size `85057080`, and the same SHA-256 above;
- `wg status` resolved the project route as
  `pi:openai-codex:gpt-5.6-sol` and showed the service running;
- the completion roles in `worksgood.toml` resolved inference, comparison, and
  Eval to `pi:openai-codex:gpt-5.6-luna` with `reasoning=low`.

This is stronger than comparing `wg --version` (which is only `0.1.0`): the
running Linux image and invoked CLI are the same inode and bytes, and that image
was installed after the named source commit. The operator had already performed
the coordinated install/restart; this worker did not replace a live executable.

## Immutable v2 phase schema and visibility boundary

`COMPLETION_REVIEW_RECEIPT_VERSION` is `2`, `FLIP_PROTOCOL` is
`prompt-reconstruction-two-phase-v2`, and each `FlipPhaseExecution` seals all of:
execution ID, phase, exact candidate binding, candidate digest, route snapshot,
input/prompt/raw-output object references and digests, parsed-output digest,
candidate-evidence digest, reveal/predecessor links, start/finish times,
executor, outcome, and record digest.

The WG-owned create-once authority marker is derived from record digest,
execution ID, phase, route-snapshot digest, raw-output digest, and the full
`task/generation/attempt/fence/candidate_sequence` binding. Publicly recomputing
content hashes cannot mint that marker.

Phase I deserializes with `deny_unknown_fields` into exactly:

- `schema`;
- `candidate_manifest_digest`;
- candidate `outputs`;
- `inspected_output_digests`.

There is no field for requirements, requirements digest, task description,
conversation, messages, or worker summary. The prompt explicitly says those
inputs are unavailable. The focused visibility test also injects a unique
original-intent canary, proves it is absent from the phase-I prompt, rejects a
forbidden `requirements` field at deserialization, then proves phase II includes
that intent.

Phase II consumes the exact `latent_hypothesis_digest` and canonical hypothesis,
then reveals the original-intent bytes, manifest and requirements digests,
dependency outputs, candidate outputs, structured validation evidence, and
inspected-output digests. Verification reloads every CAS object, requires
canonical bytes, re-renders both prompts, reprojects both raw model responses,
re-resolves the exact candidate bundle, and requires the comparison predecessor
to equal the phase-I record digest. It also requires phase-I finish not later
than phase-II start and distinct execution IDs.

## Negative and semantic-rejection proofs

`genuine_flip_proof_rejects_every_broken_execution_binding` exercises and fails
closed on a missing chain, swapped phase records, a forged route field, a
coherently re-sealed public forgery without WG execution authority, stale
generation, cross-candidate comparison, reversed chronology, one execution ID
reused for both phases, a changed phase-I output, changed comparison decision
evidence, substituted phase input, and a missing phase-input CAS object.
`freshly_persisted_flip_is_fully_reloaded_before_eval` separately corrupts a
phase-input object after the call and proves Eval is not invoked.

`completion_resilience_e2e.sh` drove a real isolated daemon and candidate binary.
Candidate A's call order was inference → comparison, its semantic rejection left
`status=in-progress`, did not land, and produced no Eval call. Candidate B's
order was inference → comparison → Eval, after which restart preserved the exact
binding and did not repeat review. Its separate exhausted-budget branch retained
a rejected candidate as `Waiting/NeedsReview`, never Done. The script's exact
recorded phase sequence was
`inference, comparison, inference, comparison, eval`.

`worker_owned_landing_turns.sh` then exercised the candidate binary's persistent
FIFO: three exact bindings advance in order, restart reloads the queue, an
expired owner is fenced, stale/changed candidates are rejected, and the queue
finishes without starvation.

## Authoritative validation command

The task's configured host-captured command is:

```text
bash scripts/validate-genuine-flip-v2-canary.sh
```

The checked-in script runs the focused proof/visibility/rejection tests; builds
and passes the exact candidate binary into both live smokes; runs
`cargo fmt --check` and `cargo clippy`; runs the Pi plugin build, self-tests,
forced compat-mismatch tripwire, and 29 Vitest tests; re-embeds with
`--no-install`; and requires an empty embed diff. This avoids treating worker
summary or log prose as validation authority. WG's completion controller records
the command identity, candidate/repository/task binding, exit/timing and bounded
stdout/stderr in a `deterministic-validation/configured/v1` object before FLIP.

Manual preflight of the same constituents passed. The plugin install reported
11 npm audit advisories (5 moderate, 5 high, 1 critical) but all requested
build/selftest/test and embed-sync commands exited zero; the advisories are not
silently represented as test failures.

## Concrete preliminary v2 receipt chain

The following fields are populated from the first real installed `wg submit`
over an immutable preliminary candidate, before the final report-only update:

| Object | CID / value |
|---|---|
| candidate manifest | `PENDING_INSTALLED_SUBMIT` |
| requirements | `PENDING_INSTALLED_SUBMIT` |
| FLIP receipt v2 | `PENDING_INSTALLED_SUBMIT` |
| phase-I record | `PENDING_INSTALLED_SUBMIT` |
| phase-I input | `PENDING_INSTALLED_SUBMIT` |
| phase-I prompt | `PENDING_INSTALLED_SUBMIT` |
| phase-I raw output | `PENDING_INSTALLED_SUBMIT` |
| latent hypothesis | `PENDING_INSTALLED_SUBMIT` |
| phase-II record | `PENDING_INSTALLED_SUBMIT` |
| phase-II input | `PENDING_INSTALLED_SUBMIT` |
| phase-II prompt | `PENDING_INSTALLED_SUBMIT` |
| phase-II raw output | `PENDING_INSTALLED_SUBMIT` |
| Eval receipt v2 | `PENDING_INSTALLED_SUBMIT` |
| inference route / execution | `PENDING_INSTALLED_SUBMIT` |
| comparison route / execution | `PENDING_INSTALLED_SUBMIT` |
| Eval route | `PENDING_INSTALLED_SUBMIT` |

## Residual risks

1. Receipt CIDs cannot be self-embedded in the exact Git tree they authenticate;
   final current-candidate CIDs must be read from the immutable graph/object
   projection after terminal completion and are recorded in the task log.
2. The end-to-end model proof depends on Pi/provider reporting and the external
   process boundary; WG proves exact route snapshots, bytes, chronology, fresh
   execution IDs, CAS integrity, and create-once adapter observation, not remote
   hardware attestation.
3. `npm ci` surfaced dependency audit advisories noted above. They are outside
   this receipt-binding canary and remain a supply-chain maintenance concern.
