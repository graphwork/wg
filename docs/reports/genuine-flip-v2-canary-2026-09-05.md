# Installed v2 genuine-FLIP bootstrap canary — 2026-09-05

## Result

The installed CLI and live project daemon were byte-identical, post-`88e79dc9`
images before this canary was submitted. The source proof-binding matrix, blind
prompt visibility test, semantic-rejection/restart smoke, landing FIFO/restart
smoke, Rust policy checks, Pi-plugin tests, and embed-sync check passed. No
`cargo install` was run by this worker.

Candidate sequence 1 deliberately exposed the unresolved receipt table to the
real installed completion path. Its genuine two-phase v2 FLIP rejected that
semantic omission, the task remained `in-progress` and unlanded, and no Eval
receipt or call was created. Candidate sequence 2 contained the concrete reject
chain but correctly could not pre-prove its own future pass. A minimal ordinary
documentation seed then completed under the same installed image and supplied
the first concrete v2 FLIP Pass→Eval Pass chain. Candidate sequence 3 is this
repaired final report; its terminal call produces the required task-local
current-candidate FLIP→Eval postcondition. Final current-candidate receipt CIDs
cannot be embedded in the Git tree they hash, so they are retained in immutable
completion objects/task logs and checked after reload with `wg show --json`.

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

## Concrete installed v2 receipt chain (candidate sequence 1)

The installed `wg done` selected manifest
`b3:cf9d862a7a0aa8b3a27a77e756155120d823d4450834b841ab653b5a9cc1c491`
for attempt `attempt-0-1`, fence 1, candidate sequence 1. It first captured the
configured validation object
`b3:19b03a15f64cef8fbc67e31f2057af75f9d1e11b4f2443b4db3e653d02959019`
(command identity
`b3:1d5439198522395c006a654bbaa5040fde8ed702c39ddd86ed713d3aef512181`,
exit 0, 225668 ms) and baseline object
`b3:8a50e7e0f60168e371807484bb9431a4130e661cb86fc5399f9164b28b142459`
(exit 0, 25 ms). Before/after HEAD, tree, and status digests were identical.

| Object | CID / value |
|---|---|
| candidate manifest | `b3:cf9d862a7a0aa8b3a27a77e756155120d823d4450834b841ab653b5a9cc1c491` |
| requirements | `b3:b128c1dda8f1766d93c244b25ae1e08286f3c42428dded0f44a21648bb98118c` |
| FLIP receipt v2 | `b3:7a25b72424ce5f47c4002bccfd6a331a864c45bb5baad8e863b6c3193b8beec0` |
| FLIP proof chain | `b3:7d89065fcff1c7bc1416aefa70d12edab92e3a841ca5d82cb72626fcb1308887` |
| phase-I record digest | `b3:5f4754adb643e10677e3da6cce4e6c876a13f6ee6b9d60a0d4865af03c4d7cd8` |
| phase-I input | `b3:85da4287a6ec711710a1b330e2a23653911a58dbdf6cf8d1349a6b13a5a58007` |
| phase-I prompt | `b3:97fe08cf5ea1a5032a28cc99d8e84757647d5e3064f647692e4800e651cae2a2` |
| phase-I raw output | `b3:24bc6dc21fb0794cf22e475306c998245fafd9cbeee1b9ce1d5e950566da95ca` |
| latent hypothesis | `b3:107bba52f19b0298d210485317fc1bae745230623979537e24729503ed2ef7db` |
| phase-II record digest | `b3:8a4e60304d68a2c5df8c38d5a3c0725a7f3a1e2365356fe5c6040553bd5f1334` |
| phase-II predecessor | exact phase-I record `b3:5f4754adb643e10677e3da6cce4e6c876a13f6ee6b9d60a0d4865af03c4d7cd8` |
| phase-II input | `b3:f3b74c2d1edc2f0d144acf9d7e0e39a63eb5508b68012508ff9052541e88608e` |
| phase-II prompt | `b3:a5ca05810c5dc0f8e15a6182a33e7568c642918c7d74f22dd233931293f34e32` |
| phase-II raw output | `b3:0e14657c947a6bf9635e0868e53a19935f3dd17e5687e2720651ba31ec1202d0` |
| revealed evidence | `b3:e0869b947dfd949d09dbf2dd1c42faee8a9fdbdc421babf4748f4007593096d8` |
| candidate evidence (same in both phases) | `b3:d88e9e7fde3bfc446d4756026d2491432fedbff69db6d8a90efd18e2c118e36a` |
| inference route / execution | `pi:openai-codex:gpt-5.6-luna` / `flip-inference:01a06fae-1946-7260-9f3a-2a1e2004c46f` |
| comparison route / execution | `pi:openai-codex:gpt-5.6-luna` / `flip-comparison:01a06fae-4ef1-7bb1-b838-f5b72a305489` |
| Eval receipt / route | **not created / not called**, because FLIP semantically rejected |

Chronology was phase-I `03:47:55.846607065Z` → `03:48:09.568916179Z`, then
phase-II `03:48:09.585355088Z` → `03:48:18.398090989Z`. The execution IDs are
distinct, phase II names the exact phase-I record, both route snapshots have CID
`b3:4efd9e65b8e23459dd2b5374f91248ae1e483b3884af3c3c0b2f173c8472f902`,
and the receipt has `receipt_version: 2`. The verdict was semantic `reject`
with findings digest
`b3:78b5f48e52a6d60b25553594b68873e7791daa61643a6c31a400a6eccde41cc0`.
The immediately following installed `wg show --json` had empty stderr, reported
`status: in-progress`, one current FLIP rejection, no completion receipt, and no
Eval activity. Thus the rejected candidate remained Not Done and unlanded.

## Successful installed bootstrap seed (Pass → Eval Pass)

The ordinary documentation prerequisite `genuine-flip-v2-seed` broke the causal
self-reference without weakening verification. It completed through installed
`wg done`, landed commit `c17e45b43069cf9f51a896df7bf35831a68f5e1e`, and
produced this current candidate sequence-3 chain:

| Object | CID / value |
|---|---|
| candidate manifest | `b3:a1b88307e51033c4df0f808847179aeebfd36a29b83897195ec6cb39bb613d78` |
| requirements | `b3:4d09af240938949b3fe6d2c37db54e1d527466e46f2b792975882840994f7906` |
| FLIP receipt v2 (Pass) | `b3:461dbab8513b217d09f14bb7ede4ef5ed0d4ab31094c8df00830552f744975e4` |
| FLIP proof chain | `b3:5a34c32fd37ff691431a9c4f6ad4e4da57ed57af579ba5ce659fb4846f5dc6bc` |
| phase-I record digest | `b3:19795e6ac9e539ac9caa4dce08717890544c5b1314f334d247b6534848b148e6` |
| phase-I input | `b3:e5f01ddd97d50b7ba9e027fcdef44f9a5536a6ea728b46ebba31495fad9575e2` |
| phase-I prompt | `b3:72d67be97e94d4e97f405dade15fc40ec25498e1eae19438537dadf231d487ab` |
| phase-I raw output | `b3:31c24da67e6f7d0020d82610fb4f3f5124f61c09bcafb1187c0fb8a6daaec74c` |
| latent hypothesis | `b3:0ddf26a1d8697bc7c598f2816395f99b9409b571b80aa639dfeada80a33c1792` |
| phase-II record digest | `b3:1679aeba272aef5c6e20c53cd9984e4f80c0b905dd876f96a70a0dbf3cf768d6` |
| phase-II input | `b3:490490a344c72a949bbf8dd3a909723ce4d33face3abf3b8e61dc3f54dd34d12` |
| phase-II prompt | `b3:a9f9b075348f1df4fc304a0a90c887874be3fad6f70ccb97214f5f53c0972714` |
| phase-II raw output | `b3:166a4f1700cff8d750d189de1dc8eb23f0abcc81d92b419ab532ad2f3dffc90d` |
| revealed evidence | `b3:e8b8908ff040c9d7cc962dbdc0dcaa3135fe6c481c817665484f678112a3ca64` |
| Eval receipt v2 (Pass) | `b3:bfc1fafec3c25a2100cee80a1321627ff18ea7d7b6e903cdcea6ce91840fe5fb` |
| completion receipt | `b3:75b2d0b241cadd452067c8a39db85c00055e990fc4cc3991147a5e2f9652ab2c` |

Phase I execution
`flip-inference:01a06fb8-d336-72f0-be9f-70d2279ed230` ran on
`pi:openai-codex:gpt-5.6-luna` from `03:59:38.806224481Z` through
`03:59:48.396343981Z`. Phase II execution
`flip-comparison:01a06fb8-f8b6-7031-a56c-f015823ed0e9` ran on the same exact
route from `03:59:48.406618532Z` through `03:59:56.053101881Z`, naming the exact
phase-I record as predecessor. Eval then ran on
`pi:openai-codex:gpt-5.6-luna` for 6918 ms. The installed projection reported
FLIP Pass before Eval Pass, both bound to the same task/generation/attempt/fence/
candidate sequence. A fresh installed `wg show genuine-flip-v2-seed --json`
returned `status=done`, both activities `candidate_state=current`,
`receipt_version=2` in both immutable objects, and **zero stderr bytes** (no
invalid-projection warning).

That successful external immutable chain is evidence available before this
candidate's review. The parent task-local postcondition is then established by
the final installed controller call itself and rechecked after reload; requiring
its final receipt CID inside the pre-review tree would be a cryptographic
self-reference, not a stronger integrity property.

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
