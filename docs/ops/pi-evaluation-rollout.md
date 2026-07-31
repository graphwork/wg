# Required deep-FLIP rollout runbook

Deep read-only system FLIP is the primary, required pre-merge feedback signal
for qualifying coding candidates. The authoritative order is:

```text
completion receipt → immutable candidate → deterministic validation
→ deep read-only FLIP → accepted FLIP report → exact-candidate merge → Done
```

Bounded grading is optional and independent. It does not precede, average with,
or satisfy FLIP. The managed `flip-required` stage therefore has
`auto_evaluate=false`, `eval_gate_all=false`, and `global_flip_enabled=true`.

## Bounded evidence sufficiency versus deep authority

The bounded lane is a no-tools grader over an automatically assembled manifest.
Before a model is invoked—and again before any returned verdict is consumed—WG
checks closed evidence locators for the exact candidate descriptor, content and
delta manifests, exact-commit patch bytes, validation receipt, task contract,
original intent, and every declared candidate-relative artifact. The assembler
expands exact immutable-commit excerpts deterministically across bounded retries;
it never reads or mounts the worker worktree and never substitutes mutable
`main`. Missing or unreadable items are `EvidenceUnavailable`; omitted or
truncated required bytes and the model's structured `insufficient_evidence`
response are `InsufficientEvidence`.

Both outcomes are evaluation infrastructure state. They consume only the
bounded retry budget, retain the source attempt/candidate in
`AwaitingAcceptance` (`PendingEval`), and can never emit
`AcceptanceRejected`, reopen/retry the source, contribute a score, or satisfy a
required FLIP gate. `wg show TASK` displays the state plus WG-generated evidence
IDs/categories; it does not log artifact paths or model/user evidence text.
Operators may repair the immutable evidence store and let the same exact route
rebundle, or run/retry the already-selected deep lane:

```bash
wg show TASK
wg evaluate run TASK --flip
```

A complete bounded bundle may still produce candidate/manifest/route-bound
pass/fail evidence. For coding or structural work that evidence is always
secondary/advisory: a requested acceptance gate is routed to deep read-only
FLIP, whose isolated observation bundle materializes the exact candidate commit
as a read-only repository. Bounded output is never averaged with deep output
and never unlocks the deep gate.

The content-addressed rollout ledger is:

```text
.wg/agency/evaluation-plane/canary-evidence.json
```

## 1. Prove the candidate binary while disabled

```bash
cargo build --locked --bin wg
WG_SMOKE_CANDIDATE_BIN="$PWD/target/debug/wg" \
  bash tests/smoke/scenarios/flip_first_required_gate.sh
WG_SMOKE_CANDIDATE_BIN="$PWD/target/debug/wg" \
  bash tests/smoke/scenarios/pi_evaluation_rollout_requires_canary_success_before_enable.sh
```

The Fake-Pi flow must prove all eight evidence classes, observation-only tools,
semantic pass and reject, timeout/malformed/process-unavailable behavior,
explicit same-record retry, restart replay, no fallback, no source retry, no
evaluation worker/build/worktree slot, and exactly-once merge. Main must remain
unchanged while pending and on reject/unavailable.

## 2. Start the managed controller

```bash
wg evaluate rollout start
wg evaluate rollout status --json
```

Required: `stage=disabled`, `auto_evaluate=false`, `eval_gate_all=false`, and
`global_flip_enabled=false`. Never edit managed keys in TOML.

## 3. Record Fake-Pi evidence

Create `fake-pi-lifecycle.json` with exact route, before/after Viz CIDs, one
source completion/verdict, and zero duplicate/stuck/never-ran/slot counters.
Then:

```bash
wg evaluate rollout advance \
  --stage fake-pi-validated --evidence fake-pi-lifecycle.json
wg service restart
wg evaluate rollout status --json
```

Restart must not duplicate a record, verdict, cost, transition, or merge.

## 4. Run the deep read-only canary (bounded is not a prerequisite)

Only the dedicated observation lane counts. A bounded summary or legacy shallow
roundtrip is not FLIP.

```bash
wg evaluate run SOURCE --flip --json > deep-record.json
```

Require an exact Pi route, a candidate-bound report, capability audit containing
only the four deep observation tools, all eight evidence classes, at least two
repository files, latent-intent/counterfactual/cross-component codes, safe
references, and no raw prompt/reasoning/secret text. Infrastructure failure is
not a semantic rejection and must use explicit same-record retry:

```bash
wg evaluate run SOURCE --flip --json
```

Record `deep-readonly-flip.json`, then:

```bash
wg evaluate rollout advance \
  --stage deep-readonly-canary-passed --evidence deep-readonly-flip.json
wg service restart
```

## 5. Live low-risk Luna **gate** canary

Use the exact available Luna route (the 2026-07-28 route was
`pi:openai-codex:gpt-5.6-luna`). Missing login/adapter/route is a loud stop; do
not fall back to Codex, Claude, another Pi model, or bounded grading.

In an isolated rehearsal graph, prove:

1. a qualifying worker completion creates one deep record and no bounded record;
2. main stays at the exact pre-candidate OID while FLIP is queued/running;
3. semantic pass at the snapshotted threshold advances main exactly once;
4. semantic fail/below-threshold retains candidate/report/worktree and leaves
   main unchanged in `AwaitingAcceptance` with repair actions;
5. timeout, malformed output, route drift, crash, and unavailable adapter leave
   main unchanged and consume only the FLIP retry budget;
6. restart before/after candidate, report write, verdict link, acceptance
   consume, and merge produces one report/verdict/merge and no duplicate cost;
7. the evidence file records before/after main OIDs and Viz CIDs, exact route,
   policy/candidate/report/merge IDs, and `gate_left_disabled=true`.

Create `flip-required-gate.json` with all controller proof booleans true. **Do
not advance the live project from an unmerged worker branch.** Carry this file
forward to the chat operator.

## 6. Operator activation—only after exact-main install

The chat/operator runs this copy-pasteable sequence only after the implementing
candidate is accepted on main:

```bash
git switch main
git pull --ff-only
cargo install --path . --locked
wg evaluate rollout status --json
wg evaluate rollout advance \
  --stage flip-required --evidence /ABS/PATH/flip-required-gate.json
wg service restart
wg evaluate rollout status --json
```

The final status must be exactly:

```text
stage=flip-required
mode=flip-required
auto_evaluate=false
eval_gate_all=false
global_flip_enabled=true
```

If the exact-main install or route check differs, stop. Do not hand-edit config.

## 7. Operator repair surfaces

`wg show TASK` and its stable JSON/TUI projection expose only bounded codes and
content IDs. For a retained candidate:

```bash
wg show TASK
wg evaluate run TASK --flip                 # retry FLIP only
wg candidate repair CANDIDATE_ID            # mint a fresh repair generation
wg candidate waive CANDIDATE_ID \
  --report REPORT_ID --reason 'OPERATOR REASON'
```

Waiver is operator-only, candidate+report bound, audited, and merges only that
exact immutable candidate. Messages and late events cannot reopen the source.

## 8. Roll back

```bash
wg evaluate rollout rollback --reason 'operator reason'
wg service restart
wg evaluate rollout status --json
```

Rollback atomically disables deep selection/gating and bounded auto-evaluation
while preserving reports, candidates, rollout evidence, and already accepted
history. It never rewrites an accepted source outcome.
