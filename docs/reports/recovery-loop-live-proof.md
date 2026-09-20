# Recovery-loop live proof — attempt, receipts, and honest gaps

**Task:** `prove-the-recoverable`
**Recovery commit under test:** `7f7e600c` — *feat: recoverable review rejection
resumes the same node in place (implement-milestone-2-of)*
**Date:** 2026-09-20/21
**Report author:** `agent-157` (trusted worker)

## 0. Verdict (read this first)

**The recoverable-rejection loop was NOT observed on a real dispatch, and the
first Validation criterion is therefore NOT met.** This report does not paper
over that: it documents two independent reasons the loop was unreachable in
this environment, the live experiment that was run anyway, the real receipts,
and the classification output that shows what *would* have happened.

Two concrete blockers, both reproducible:

1. **The running project cannot enter the recovery path.** Recovery is reachable
   only under strict completion-review policy
   (`src/commands/completion_submit.rs`: `handle_semantic_rejection` is called
   from the `FlipRejected | EvalRejected` arm only after the
   `!config.agency.completion_review_strict` early-return). The project document
   `/home/bot/wg/worksgood.toml` sets `completion_review_strict = false`, so the
   effective policy is `mode=advisory`, and a rejection only prints a warning.
   A worker cannot change this: `wg config`, `wg service`, and the other
   admin command families are refused with
   `worker_control.admin_operation_refused` (see `src/worker_cli.rs`).
2. **The deployed binary predates the recovery code.** `/home/bot/.cargo/bin/wg`
   was built `2026-09-20 22:09:29`; the recovery commit `7f7e600c` is dated
   `2026-09-20 23:05:55`. `strings` confirms the deployed binary has the
   milestone-1 calibrated FLIP prompt
   (`prompt-reconstruction-two-phase-v3`) but **none** of the milestone-2
   recovery strings (`COMPLETION REVIEW RECOVERY ROUND`, `review-semantic-recovery`).
   The running daemon therefore cannot execute the recovery code even if strict
   policy were enabled. Recovery has never been deployed.

To nevertheless exercise the *real* machinery, I built the recovery commit and
stood up a **separate, isolated, strict-policy WG instance** with its own daemon
and real Pi workers (the repo's sanctioned isolated `$HOME`/`--dir` pattern),
published nine real tasks, and let the daemon dispatch them. All nine candidates
passed FLIP fidelity + Eval acceptance; none produced a semantic rejection, so
none entered `Repairing`. One probe hit the **deterministic** repair loop (a
different mechanism) and auto-resolved in 32 s.

The production classifier, run over the one real rejection this repository has
actually recorded, returns **`Recoverable`** — so the trigger logic is sound;
it simply never fired live, and the deployed environment could not have run it.

## 1. What I expected to observe

The task asked for a real task whose deliverable is legitimately defective
enough for the semantic gate to reject, then the full arc:

`rejection -> Repairing + recovery_round >= 1 -> bounded corrective checkpoint
on the same attempt/session/fence -> auto-resumed Timer wait (no operator) ->
worker receives the corrective prompt -> clean pass or NeedsAttention`.

I expected a rejection because the design (`docs/design-review-recovery.md` §3)
is built around exactly this. What the live system actually showed is that the
*trigger* is much narrower in practice than the implementation suggests.

## 2. Environment findings (reproducible)

### 2.1 Effective policy is advisory, not strict

```
$ wg status | grep -i "Completion review policy"
Completion review policy: mode=advisory, applicability=required-for-structural-deliverables; otherwise-advisory, evaluator-threshold=0.70, FLIP-policy=required-strict-when-persisted-in-hard-gate, FLIP-threshold=0.70
```

`/home/bot/wg/worksgood.toml:14: completion_review_strict = false`.

Under advisory mode, `src/commands/completion_submit.rs` takes the
`FlipRejected | EvalRejected if !config.agency.completion_review_strict` arm,
which prints `WARNING: ... Review policy is advisory ...` and returns `Ok(())`.
`handle_semantic_rejection` is never reached, so neither `park_semantic_recovery`
nor any `Repairing` disposition can be written.

### 2.2 Workers cannot change the policy

```
$ wg capabilities
Worker control mode: trusted
Restrictions: normal local graph coordination is allowed; terminal completion
remains own-attempt, receipt-backed, and fenced; service/admin and immutable
evidence internals remain protected

$ wg config
Error: worker_control.admin_operation_refused: command is outside trusted local graph coordination
```

`src/worker_cli.rs` refuses `Config`, `Service`, `Setup`, `Profile`, etc. even
for trusted workers. Enabling strict policy therefore requires an operator, not
an agent. (I did **not** edit the shared `worksgood.toml` to bypass this.)

### 2.3 The deployed binary lacks the recovery code

Evidence: `docs/reports/recovery-loop-live-proof/deployed-binary.txt`.

```
shared binary: /home/bot/.cargo/bin/wg
shared mtime:  2026-09-20 22:09:29
recovery commit: 2026-09-20 23:05:55  7f7e600c
strings[prompt-reconstruction-two-phase-v3]: shared=2 built=2
strings[COMPLETION REVIEW RECOVERY ROUND]:  shared=0 built=1
strings[review-semantic-recovery]:          shared=0 built=1
```

So the daemon serving the graph has the calibrated reviewer but not the
in-place recovery. `wg done` on any task in this project runs the pre-recovery
binary.

## 3. The isolated strict-policy live run

Because the shared project is permanently advisory and its binary is stale, I
ran the experiment in an isolated WG instance built from the recovery commit.
This follows the repository's own isolated-instance pattern (AGENTS.md
"wgrun"/pilot pattern): a scratch project root, a separate `.wg`, a separate
`WG_GLOBAL_DIR`, and the real `$HOME` kept only so Pi keeps its own auth. It
does not touch the shared graph.

### 3.1 Exact setup commands

```bash
cargo build --locked --bin wg          # from worktree at 7f7e600c, 2m12s
BIN=<worktree target>/debug/wg         # contains "COMPLETION REVIEW RECOVERY ROUND"

SCRATCH=/tmp/recovery-live-proof
PROJ=$SCRATCH/project
export HOME=/home/bot WG_GLOBAL_DIR=$SCRATCH/global   # Pi keeps auth; WG state isolated
unset WG_DIR WG_TASK_ID WG_AGENT_ID ...               # drop worker identity
cd "$PROJ"
git init -q -b main && git commit -qm base
"$BIN" init
cp /home/bot/wg/worksgood.toml "$PROJ/worksgood.toml"
sed -i 's/^completion_review_strict = false/completion_review_strict = true/' "$PROJ/worksgood.toml"
sed -i 's/^max_agents = 8/max_agents = 2/' "$PROJ/worksgood.toml"
"$BIN" --dir "$PROJ/.wg" status | grep "Completion review policy"
# => mode=strict

setsid bash -c "cd '$PROJ' && exec '$BIN' --dir '$PROJ/.wg' service start \
  --no-chat-agent --force --max-agents 2" &
"$BIN" --dir "$PROJ/.wg" service status
# => Service: running (PID ...); Dispatcher: max_agents=2, executor=pi,
#    model=pi:lunaroute/deepseek-4.1-flash-background-background
```

Evidence: `docs/reports/recovery-loop-live-proof/isolated-instance.txt`.

### 3.2 Nine real dispatched probes

Each probe was authored with `wg add ... --id <id>` (draft), then
`wg publish <id> --only`; the daemon dispatched it to a real Pi worker
(`agent-1` … `agent-9`) in an isolated worktree. Every probe was designed so a
semantic rejection would be *legitimate* if the delivered bytes failed the
stated acceptance requirements. Designs, in order of increasing strictness:

| id | design intent | expected rejection class if defective |
|----|---------------|----------------------------------------|
| `recovery-probe-live` | 5 exact `##` sections, digest token, JSON keys; fully compliant | fidelity/coverage |
| `-2` | 11 exact constraints (exact heading set, exact bullet counts, exact fenced block, exemption from H1/H3, no `TODO`) | coverage/format |
| `-3` | 12-row exact-copy Markdown matrix + 64-hex nonce in two places + exact structure | fidelity/coverage |
| `-4` | **controlled-vocabulary ban** (no `recover/repair/reject/checkpoint/resume/gate/review`) + exact structure | fidelity (contradicts revealed constraint) |
| `-5` | tight per-section word windows (40–45, 45–50, 25–30, 20–25, 20–25) + banned vocabulary | coverage/length |
| `-6` | `-5` spec on a **low-reasoning** worker (`--reasoning low`) | coverage/length |
| `-7` | named runnable check `recovery-check.sh` required in `## Validation` but not host-captured | evidence-gap |
| `-8` | real Rust implementation whose `## Validation` names `cargo test --lib` / `cargo fmt --check` / `cargo clippy` | evidence-gap (pre-calibration) |
| `-9` | self-verifying sections: each must state its own exact word count | count mismatch |

### 3.3 Results

All nine tasks ended `done` / `Landed` with **no `completion_repair` at all**
(no `Repairing`, no `recovery_round`). Every one produced a two-phase FLIP pass
and an Eval pass with zero findings. Raw receipts per probe are in
`docs/reports/recovery-loop-live-proof/recovery-probe-live*.review.txt`; the
state transitions are in `transitions-*.log`.

```
23:31:30 in-progress|agent-1|...        recovery-probe-live   -> done/landed
23:33:32 in-progress|agent-2|...        recovery-probe-live-2 -> done/landed
23:49:31 in-progress|agent-3|...        recovery-probe-live-3 -> done/landed
23:50:37 in-progress|agent-4|...        recovery-probe-live-4 -> done/landed
23:52:23 in-progress|agent-5|...        recovery-probe-live-5 -> done/landed
23:53:57 in-progress|agent-6|...        recovery-probe-live-6 -> done/landed
23:56:27 in-progress|agent-7|...        recovery-probe-live-7 -> done/landed
23:59:27 in-progress|agent-8|...        recovery-probe-live-8 -> deterministic repair -> done/landed
00:00:33 in-progress|agent-9|...        recovery-probe-live-9 -> done/landed
```

The worker proved genuinely capable: for `-5` and `-6` I independently verified
the per-section word counts (44/50/28/23 accompanying a 40–45/45–50/25–30/20–25
spec) and the banned-substring scan (no hits) — the reviewer's pass was
*correct*. For the controlled-vocabulary probe `-4` the worker described the
whole arc without ever using one of the ten banned substrings. These are not
reviewer misses; the candidates were compliant.

### 3.4 The one repair event was deterministic, not semantic

Probe `-8` is the only probe that left `in-progress` before landing. Its first
`wg done` failed the **built-in deterministic** baseline, which wrote the
*existing* deterministic repair state, not the semantic recovery state:

```
23:59:35 in-progress|agent-8|repairing|None|deterministic-check-failed|1|-|None
00:00:07 in-progress|agent-8|resolved |None|deterministic-checks-passed|1|-|None
00:00:16 done|None|None|None|None|None|-|landed
```

`docs/reports/recovery-loop-live-proof/probe8-state-*.json` shows
`completion_repair.disposition=repairing` / `reason_code=deterministic-check-failed`
/ `opportunities_used=1`, with **`recovery_round` absent** — this is
`record_deterministic_repair_failure`, not `record_semantic_repair_recovery`.
It was resolved by the worker re-running `wg done` (`deterministic-checks-passed`),
not by a semantic rejection. It is recorded here to avoid conflating the two
mechanisms.

## 4. A real live rejection, and the classification output

The repository has exactly one recorded live Eval **semantic rejection**:
`implement-flip-fidelity` (rejected under the *pre-calibration* reviewer,
`failure_class=semantic_rejection`). Its three findings are in
`docs/reports/recovery-loop-live-proof/real-live-rejection-implement-flip-fidelity.txt`:

```
finding [completion.missing_authoritative_runtime_evidence]: ... cargo test --lib ...   (evidence: cargo test --lib)
finding [completion.missing_authoritative_runtime_evidence]: ... cargo fmt --check ...  (evidence: cargo fmt --check)
finding [completion.missing_authoritative_runtime_evidence]: ... cargo clippy ...       (evidence: cargo clippy --all-targets --all-features -- -D warnings)
```

I ran the **production** `classify_semantic_rejection` over those exact findings
via the committed evidence test `tests/recovery_classification_evidence.rs`:

```
$ cargo test --locked --test recovery_classification_evidence -- --nocapture
REAL_LIVE_MULTI_CHECK_REJECTION => Recoverable
SINGLE_EXACT_COMMAND_EVIDENCE_GAP => EvidenceGap
SUBSTANTIVE_GAP => Recoverable
DEFECT => Irrecoverable(DefectOrIncompleteImplementation)
MIXED => Recoverable
```

Output: `docs/reports/recovery-loop-live-proof/classification-output.txt`.

**This is the most important finding of the task.** The one rejection the
system actually produced classifies as **`Recoverable`**, not as the excluded
`EvidenceGap`, because `is_authoritative_runtime_evidence_gap`
(`src/completion_review.rs`) returns true only when *every* reserved-code finding
proposes the **same single exact command**. A multi-check evidence gap
(three distinct missing `cargo` envelopes) therefore falls through to the
recoverable-by-default branch. So:

- the loop is not dead code: had `7f7e600c` been deployed **and** strict policy
  enabled, that real rejection would have entered `Repairing`, written a
  corrective checkpoint, and auto-resumed;
- but neither condition holds in the running system, so it has never happened.

The *single*-command evidence-gap case (the design's "one exact missing check")
does classify `EvidenceGap` and is excluded, as designed.

## 5. Why the loop did not trigger — synthesis

1. **Unreachable at the policy layer (primary).** Strict policy off + worker
   admin refusal means no agent, and no real dispatch in this project, can ever
   reach `handle_semantic_rejection`.
2. **Unreachable at the binary layer.** The deployed `wg` predates the recovery
   commit; its `wg done` cannot write `Repairing`/`recovery_round` regardless of
   policy.
3. **When forced (isolated strict instance), no rejection occurred.** Nine real
   dispatched probes, including deliberately over-specified, exact-copy,
   negative-vocabulary, word-window, and named-runnable-check designs, were all
   judged compliant by the calibrated v3 reviewer. The calibration deliberately
   removed evidence-ceremony rejections, and the worker is strong enough to
   satisfy explicit specs, so no substantive gap appeared. The only repair event
   was the deterministic baseline path.
4. **The historical rejection class is either excluded or non-reproducible.**
   The single-command evidence gap is excluded by design; the actual recorded
   multi-check rejection is `Recoverable` but was produced by the *uncalibrated*
   reviewer and could not be reproduced post-calibration (probe `-8` named the
   same three `cargo` checks and passed).

## 6. Honest gaps — what did not work as designed

- **Validation criterion 1 is unmet.** No real dispatched task reached
  `Repairing` with `recovery_round >= 1`. There is no checkpoint text to quote
  and no auto-resumed `Timer` wait to observe, because the park was never
  written. Checkpoints observed in this work belong to the **deterministic**
  repair path (`LandingPending` for probe `-1`, `deterministic-check-failed` for
  probe `-8`), not semantic recovery.
- **The isolated run cannot fully substitute for the shared graph.** It uses the
  same binary, daemon code, reviewer routes, and real Pi workers, but a distinct
  graph/keystore. It is a real dispatch, not the production graph.
- **Worker strength may mask the loop.** Using the strong default Pi worker
  (`deepseek-4.1-flash-background-background`, reasoning high/low) produced no defective
  candidates. A deliberately weak worker might, but the available Lunaroute
  catalog has no materially weaker text model, and selecting one to fail would
  edge toward manufacturing a defect rather than observing one.
- **One reviewer-leniency data point, not a clean defect.** Probe `-9`'s
  self-reported `## Gaps` word count appears off by one or two depending on
  whether the trailing `END-OF-FIXTURE` marker counts as part of the section;
  word counting is itself ambiguous, so this is not asserted as a gate failure.
  It is noted only because the reviewer did not challenge it.
- **No gate was weakened, and no defective candidate was accepted.** The
  isolated instance used strict policy (a strengthening) and every accepted
  candidate was independently checked to be compliant. The shared project's
  config and binary were left untouched.

## 7. What the operator must do next (to actually prove the loop)

1. **Deploy the recovery code.** `cargo install --path . --locked` from
   `7f7e600c` (or later), then restart the service. Until then the running
   `wg` cannot execute `7f7e600c`'s recovery.
2. **Enable strict review for the proof window** — either
   `completion_review_strict = true` in `worksgood.toml`, or (preferable for
   blast radius) run the proof in an isolated strict instance as done here.
3. **Inject a *genuine* substantive gap that the calibrated Eval will reject.**
   Post-calibration, evidence-ceremony gaps are no longer rejected. A legitimate
   recoverable trigger is a candidate that *omits a required deliverable* while
   otherwise passing the deterministic baseline — a coverage gap, not an
   evidence gap. Because the default worker is strong, the operator may need to
   pin a weaker executor for the probe task.
4. **Watch for the multi-check subtlety in §4.** If the intended policy is
   "any all-reserved evidence-gap rejection is excluded", the current
   `is_authoritative_runtime_evidence_gap` single-command requirement is a gap:
   `cargo test` + `cargo fmt` + `cargo clippy` missing together currently routes
   into the recovery loop rather than the contract-correction help path. Decide
   whether that is intended and, if not, fix it (or document it) in a follow-up.

## 8. Reproduction / cleanup

All commands are in §3.1. The isolated instance lives entirely under
`/tmp/recovery-live-proof` (nothing was written into the shared `.wg`). Stop it
with:

```bash
HOME=/home/bot WG_GLOBAL_DIR=/tmp/recovery-live-proof/global \
  "$BIN" --dir /tmp/recovery-live-proof/project/.wg service stop --force --kill-agents
rm -rf /tmp/recovery-live-proof
```

The committed evidence is `docs/reports/recovery-loop-live-proof/`:

- `isolated-instance.txt` — binary, commit, effective strict policy, daemon
- `deployed-binary.txt` — deployed-vs-built string/mtime proof
- `classification-output.txt` — production classifier over the real rejection
- `real-live-rejection-implement-flip-fidelity.txt` — the real live findings
- `transitions-probe*.log` — dispatch/status transitions for all nine probes
- `recovery-probe-live*.review.txt` — FLIP/Eval receipt digests and verdicts
- `probe8-state-*.json` — the deterministic (non-semantic) repair snapshots
- `../recovery_classification_evidence.rs`-adjacent test:
  `tests/recovery_classification_evidence.rs`
