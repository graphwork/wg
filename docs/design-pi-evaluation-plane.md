# Pi-first evaluation and deep-readonly FLIP plane

**Status:** implementation-ready design; no production code in this change  
**Date:** 2026-07-26  
**Depends on:** [Simplified authoritative task lifecycle](design-simplified-task-lifecycle.md), especially §§10–11  
**Default rollout state:** disabled

## 1. Decision

WG will replace eager `.evaluate-*` and `.flip-*` task satellites with **lazy, attempt-bound evaluation records**. A record can be created only after a real source execution reaches candidate completion, its exact candidate is durably checkpointed, and the pinned policy selects evaluation. Evaluation runs in a dedicated agency lane, not in the worker/build lane.

The plane deliberately exposes two different products:

1. **Bounded evaluation** is the routine/default product. It is one ephemeral, no-tools Pi call over one content-addressed evidence bundle. It produces a small strictly parsed verdict. It cannot inspect the live graph or filesystem.
2. **Deep-readonly FLIP** is selective. An operator or a high-risk policy must request it. It performs blind latent-intent reconstruction, then a separate comparison/counterfactual phase with observation-only access to graph, intent, dependencies, artifacts, validation, traces, and redacted effective configuration. It cannot mutate the graph, source, configuration, session, or candidate; invoke shell commands; make arbitrary network calls; or author as the source agent.

Pi is the recommended and initial adapter. The domain interface is executor-neutral: an explicitly selected Codex or Claude evaluation adapter can implement the same contract. Adapter and exact route are persisted before execution. Failure of one adapter **never** selects another adapter or route implicitly.

Routine evaluation is advisory. A selected hard gate holds the source in the lifecycle design's `Finalizing`/`AwaitingAcceptance` stage. Evaluator infrastructure failure changes only the evaluation record. It never reopens, retries, rescues, fails, or completes the source by itself.

## 2. Why the present shape failed

This is not a green-field cleanup. Each rule below closes an observed failure chain.

| Observed failure | Source/history evidence | New invariant | Permanent regression scenario |
|---|---|---|---|
| **Eager satellites existed before useful work.** Publishing built `.assign-X → X → .flip-X → .evaluate-X` as real task edges. | `src/commands/eval_scaffold.rs:1-8,156-318` explicitly describes and implements publish-time eager rows. `src/commands/service/coordinator.rs:1717-1784` is a second catch-all creator. | **E1 — lazy only:** publication, opening the graph, dispatch readiness, and source admission create zero evaluation/FLIP records. | `pi_eval_lazy_candidate_only` |
| **Non-executed tasks acquired evaluation work.** The coordinator catch-all excludes paused/system/human/abandoned tasks, but not ordinary open, never-spawned, deferred, or launch-failed work. The command also historically accepted failed sources. | `src/commands/service/coordinator.rs:1751-1775`; `src/commands/evaluate.rs:238-248`. Commit `56abb531` documents the `resolve-prophage-source` zombie evaluator that repeatedly tried to evaluate an open source. | **E2 — proof of execution and candidate:** automatic creation requires `AttemptRunning`, a completion intent, writer quiescence, and `CandidateCheckpointed` for that same attempt. Failure/cancel/skip/open/deferral is insufficient. | `pi_eval_never_ran_matrix` |
| **A removed dependency edge left an evaluator runnable against an open source.** | Parent of `56abb531` removed the evaluator's `after` edge for failed sources. The fix preserves the edge and adds a no-charge eligibility check; see `tests/smoke/scenarios/evaluation_satellite_not_charged_against_open_task.sh`. | **E3 — no dependency emulation:** evaluations are records, not graph tasks, and therefore have no ordinary `after` edge to strip or satisfy. Eligibility is an exact source reference. | `pi_eval_no_zombie_after_retry` |
| **Admission backpressure was charged as a spawn failure and could trip the circuit breaker.** This task's own pre-fix history repeatedly recorded `build-heavy admission budget full (1/1)` as spawn failures. | Commit `d4dac15e` introduced typed `AdmissionDeferral`, routed it around `record_spawn_failure`, and added `tests/smoke/scenarios/admission_deferral_backpressure.sh:67-106`. | **E4 — denial is not an attempt:** source admission deferral creates neither a source attempt nor evaluation work; agency-lane deferral creates no runner attempt and consumes no retry/breaker budget. | `pi_eval_admission_deferral_neutral` |
| **Source attempt and evaluator pipeline drifted across retry.** Attempt 2 waited while satellites/verdicts still named attempt 1; exact reconciliation correctly refused them and then stalled. | `docs/reports/eval-retry-pipeline-drift.md:6-20`, commit `1ade043d`, and `tests/smoke/scenarios/eval_retry_pipeline_drift.sh`. Current repair comments at `src/eval_lifecycle.rs:339-365` explain the old re-derivation bug. | **E5 — exact immutable binding:** job, bundle, runner attempt, and verdict repeat `(task, generation, source_attempt, finalization_round, candidate_digest, policy_digest, route_digest)`. Ambient config and “latest” are never consulted. | `pi_eval_attempt_candidate_route_fence` |
| **Evaluator work shared scheduling capacity with workers.** Dot tasks are now `GraphOnly`, which avoids build admission, but they still traverse the normal ready loop, stop at `summary.spawned >= slots_available`, and increment the same `summary.spawned`. | `src/disk_sentinel.rs:47-100` is the partial build-class fix; `src/commands/service/coordinator.rs:4435-4461,4604-4672` shows the shared ready list, normal slot ceiling, and counter. | **E6 — physically separate lane:** evaluation has its own queue, leases, registry, concurrency, timeout, and metrics. It cannot consume a worker or build-heavy permit. | `pi_eval_agency_lane_independence` |
| **Silent/slow evaluator calls looked dead or could run too long.** | Commit `ad792f4f` and `inline_evaluator_silent_heartbeat.sh` added supervision around the old inline process. | **E7 — bounded runner:** every attempt has an external deadline, progress timestamps from Pi events, an input/output budget, and a terminal infrastructure disposition. | `pi_eval_timeout_budget_visible` |
| **A durable verdict could be delivered twice or after restart.** | `live_pi_evaluation_verdict_restart.sh` and the current `DurableEvalVerdict` code established create-once evidence and restart linking. | **E8 — write once/link once/consume once:** duplicate identical delivery is a no-op; conflicting content at the same semantic key is quarantined; consumption is a fenced CAS. | `pi_eval_duplicate_delivery_exact_once` |
| **Route repair and fallback risked changing the evaluator that produced a verdict.** | `pending_eval_lifecycle_route_recovery.sh` and `eval_retry_pipeline_drift.sh` pin complete routes. | **E9 — no silent route or executor fallback:** a retry uses the same persisted adapter/route/reasoning. An operator reroute creates a new audited route generation and can never relabel old output. | `pi_eval_no_cross_executor_fallback` and `pi_eval_route_drift_rejected` |

The current fixes are valuable evidence, not the final architecture. They made eager satellites less dangerous. Removing the satellites removes the class of edge surgery, ordinary-task retries, worker-slot coupling, and inferred source eligibility altogether.

## 3. Upstream Pi contracts researched

The contracts in this section were checked against installed `@earendil-works/pi-coding-agent` **0.82.0**, not inferred from WG's adapter. Upstream URLs below are the canonical source links shipped by Pi.

### 3.1 Documents and examples read

- [Pi README and CLI reference](https://github.com/earendil-works/pi-mono/blob/main/packages/coding-agent/README.md)
- [JSON event stream](https://github.com/earendil-works/pi-mono/blob/main/packages/coding-agent/docs/json.md)
- [RPC protocol](https://github.com/earendil-works/pi-mono/blob/main/packages/coding-agent/docs/rpc.md)
- [SDK](https://github.com/earendil-works/pi-mono/blob/main/packages/coding-agent/docs/sdk.md)
- [Extensions](https://github.com/earendil-works/pi-mono/blob/main/packages/coding-agent/docs/extensions.md)
- [Sessions](https://github.com/earendil-works/pi-mono/blob/main/packages/coding-agent/docs/sessions.md), [session format](https://github.com/earendil-works/pi-mono/blob/main/packages/coding-agent/docs/session-format.md), and [compaction](https://github.com/earendil-works/pi-mono/blob/main/packages/coding-agent/docs/compaction.md)
- [Security](https://github.com/earendil-works/pi-mono/blob/main/packages/coding-agent/docs/security.md), [containerization](https://github.com/earendil-works/pi-mono/blob/main/packages/coding-agent/docs/containerization.md), [settings/retry](https://github.com/earendil-works/pi-mono/blob/main/packages/coding-agent/docs/settings.md), and [providers/auth resolution](https://github.com/earendil-works/pi-mono/blob/main/packages/coding-agent/docs/providers.md)
- [SDK read-only tools example](https://github.com/earendil-works/pi-mono/blob/main/packages/coding-agent/examples/sdk/05-tools.ts), [full-control/no-discovery example](https://github.com/earendil-works/pi-mono/blob/main/packages/coding-agent/examples/sdk/12-full-control.ts), and [session-runtime example](https://github.com/earendil-works/pi-mono/blob/main/packages/coding-agent/examples/sdk/13-session-runtime.ts)
- [Structured-output terminating tool](https://github.com/earendil-works/pi-mono/blob/main/packages/coding-agent/examples/extensions/structured-output.ts), [read override/access control](https://github.com/earendil-works/pi-mono/blob/main/packages/coding-agent/examples/extensions/tool-override.ts), [protected paths](https://github.com/earendil-works/pi-mono/blob/main/packages/coding-agent/examples/extensions/protected-paths.ts), [permission gate](https://github.com/earendil-works/pi-mono/blob/main/packages/coding-agent/examples/extensions/permission-gate.ts), and [sandbox example](https://github.com/earendil-works/pi-mono/blob/main/packages/coding-agent/examples/extensions/sandbox/index.ts)

### 3.2 Contract ledger

| Pi contract | Consequence for WG |
|---|---|
| JSON mode emits LF-delimited events beginning with a session header. `turn_end.message` carries the completed assistant message. | A parser must retain event order and take usage from `turn_end`, not repeated partial messages. |
| RPC is strict LF-delimited JSONL. Generic readers that split U+2028/U+2029 are explicitly non-compliant. Commands may carry `id`; responses echo it. | The adapter uses byte buffering and `read_until(b'\n')`, request IDs, and bounded frame size. It never uses a Unicode line reader. |
| A successful RPC `prompt` response means accepted/queued/handled, **not** successful completion. Failures after acceptance arrive in message/events. | A runner cannot treat `{"success":true}` as a verdict. It waits for `agent_settled`, then inspects final assistant state. |
| `agent_end` is only a low-level run boundary. Retry, compaction, or queued work may follow. `agent_settled` means Pi will not continue automatically. | Completion is keyed to `agent_settled`, not `agent_end`. Before prompting, WG sends `set_auto_retry(false)` so WG owns the visible retry budget. |
| RPC `get_state` returns the full selected model and thinking level. `get_last_assistant_text` and `get_session_stats` are explicit commands. | Before the prompt, WG verifies reported provider/model/thinking against the persisted route. After settlement it retrieves final text and session totals with correlated requests. |
| Assistant messages report `provider`, `model`, `stopReason` (`stop`, `length`, `toolUse`, `error`, `aborted`), optional `errorMessage`, and usage `{input, output, cacheRead, cacheWrite, totalTokens, cost{...,total}}`. Tool results may carry nested usage. | The invocation receipt stores these exact Pi fields. `length`, `error`, `aborted`, missing usage, or a route mismatch cannot yield an accepted verdict. Usage is summed once per completed turn plus declared nested tool usage. |
| `--no-tools` disables all tools. `--no-builtin-tools` disables built-ins while preserving explicit extension/custom tools. Built-ins are `read`, `bash`, `edit`, `write`, `grep`, `find`, and `ls`; the documented read-only set is `read,grep,find,ls`. | Bounded evaluation uses `--no-tools`. Deep FLIP uses `--no-builtin-tools` plus only WG's named observation tools. These modes are intentionally different. |
| Default resource loading can discover global/project extensions, skills, templates, context files, settings, and system prompt additions. `--no-extensions` still permits explicitly named `-e` extensions. Noninteractive project trust does not itself disable global resources. | Both products disable discovery for extensions, skills, prompt templates, and context files and use `--no-approve`. Bounded loads no extension. Deep FLIP loads one version-locked extension by absolute path. The cwd is an empty runner directory. |
| `--no-session` makes the invocation ephemeral. Persistent sessions are append-only JSONL trees and can contain compaction summaries, branches, custom messages, and model changes. | Evaluation does not continue a source or chat session and cannot inherit its compaction/context. Original conversation is copied as provenance-tagged evidence, never opened as a writable Pi session. |
| Pi RPC 0.82.0 has no response-schema/`response_format` command. The upstream structured-output example achieves typed output by registering a tool and returning `terminate: true`. | “Structured” bounded output means a strict JSON document in the final assistant text, parsed with unknown-field denial; it does **not** claim provider-enforced structured output. Deep FLIP may use a terminating `submit_flip_verdict` tool because that product intentionally has an extension/tool surface. |
| Extensions run with the full permissions of the Pi process. Project trust is not a sandbox. Path filters are not a complete OS boundary. Pi recommends containers/VMs/policy sandboxes for untrusted unattended work. | The observation extension is necessary but not sufficient. Deep FLIP runs with a read-only evidence view, empty writable temp, no source/config mount, no shell/process tool, and a deny-by-default child-process policy. If this containment cannot be established, deep FLIP is `unavailable`, never degraded to bounded evaluation or an unrestricted “read-only” shell. |
| Pi owns provider login, catalog availability, endpoints, OAuth/API keys, and provider errors. | WG passes only persisted provider/model/thinking. It does not copy credentials into evidence, pre-resolve endpoints, or reinterpret a credential error as a semantic reject. |

### 3.3 Chosen Pi invocation

The first Pi adapter uses a fresh `pi --mode rpc --no-session` process per runner attempt. RPC is preferred over WG's current JSON one-shot (`src/service/llm.rs:958-1040`) because it gives an explicit preflight `get_state`, request correlation, an unambiguous `agent_settled` boundary, `abort`, and post-run stats.

Bounded invocation flags are equivalent to:

```text
pi --mode rpc --no-session --no-tools \
   --no-extensions --no-skills --no-prompt-templates --no-context-files \
   --no-approve --provider <pinned-provider> --model <pinned-model> \
   --thinking <pinned-level>
```

Deep FLIP replaces `--no-tools` with:

```text
--no-builtin-tools --tools <exact WG observation list> \
--no-extensions -e <absolute version-locked deep-readonly extension>
```

It still disables all other resources. `PI_SKIP_VERSION_CHECK=1` and `PI_TELEMETRY=0` prevent unrelated startup traffic. WG does **not** use `PI_OFFLINE=1`, because model transport must remain available and Pi owns it.

Protocol sequence:

1. spawn with exact argv; start wall/input/output budget guards;
2. `get_state` and compare provider, model, and thinking to the record;
3. `set_auto_retry(false)` so one WG runner attempt is not an opaque stack of Pi retries;
4. `prompt` with a unique request ID;
5. require the correlated acceptance response;
6. stream bounded events until `agent_settled` or deadline/abort;
7. request `get_last_assistant_text` and `get_session_stats`;
8. validate every assistant message route, stop reason, usage, and (for bounded mode) absence of tool events;
9. strict-parse the product schema; and
10. terminate and reap the child.

A future same-process TypeScript SDK adapter may replace the transport without changing the domain contract. It must preserve these tests and capabilities.

## 4. Domain model and lazy creation

### 4.1 Smallest safe first persistence shape

The target architecture in the lifecycle design is a hidden `EvaluationRecord`, not an ordinary task. The smallest safe first implementation stores a compact, serde-defaulted list of evaluation records on the source task's compatibility projection while keeping large bundles, transcripts, and verdicts in content-addressed files:

```rust
Task {
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    evaluation_records: Vec<EvaluationRecord>,
    // existing fields...
}
```

This gives atomic creation/link/consumption with the source under the existing `graph.lock`, without adding a new graph task/node kind or a multi-file transaction. It is a separate state domain: records have no `Status`, `after`, `before`, assignment, worktree, message inbox, retry command, or worker identity.

Large immutable objects live under:

```text
.wg/agency/evaluation-plane/cas/b3/<digest>
.wg/agency/evaluation-plane/verdicts/<verdict-id>.json
.wg/agency/evaluation-plane/raw/<runner-attempt-id>.jsonl   # filtered protocol receipt
```

When the lifecycle ledger becomes the sole projection source, the same record events move into it and `Task.evaluation_records` becomes a derived cache. No public ID or evidence schema changes. `wg show --internal` and Viz may render virtual aliases such as `.evaluate-T@g2/a4`, but aliases are not addressable tasks.

### 4.2 Record

```rust
struct EvaluationRecord {
    schema: u16,
    evaluation_id: String,
    product: EvaluationProduct, // Bounded | DeepReadonlyFlip
    source: SourceCandidateRef,
    policy: EvaluationPolicySnapshot,
    route: EvaluationRouteSnapshot,
    evidence_request: EvidenceRequest,
    bundle_cid: Option<String>,
    state: EvaluationState,
    runner_attempts: Vec<EvaluationRunnerAttempt>,
    evidence_ids: Vec<String>,
    consumed_verdict_id: Option<String>,
    created_by_event: String,
    created_at: String,
}

struct SourceCandidateRef {
    task_id: String,
    generation: u64,
    source_attempt_id: String,
    source_fence: u64,
    finalization_round: u32,
    candidate_digest: String,
    candidate_manifest_digest: String,
    dependency_revision_digest: String,
}
```

`evaluation_id` is deterministic:

```text
b3("wg-evaluation-v1\0" || product || canonical(SourceCandidateRef) ||
   policy_digest || route_digest)
```

The same trigger replay therefore finds the same record. A second product or changed explicit policy/route has a different ID and retained provenance.

### 4.3 Creation predicate

Automatic creation is legal only when all predicates are present in the locked source snapshot:

1. a real source `AttemptReserved` exists;
2. that same attempt reached `LaunchPermitted` and `AttemptRunning` (or the equivalent authenticated remote-run receipt);
3. its first disposition intent is `Complete`, not `Fail`, `Park`, `Cancel`, or `Skip`;
4. all candidate writers are quiescent/fenced;
5. `CandidateCheckpointed` exists for the same attempt/fence/finalization round;
6. deterministic pre-evaluation validation completed as required by policy;
7. the generation's pinned policy selects this product; and
8. no record with the deterministic ID exists.

Therefore these produce **no** automatic evaluation work: draft/unpublished, open, dependency-blocked, admission-deferred, never claimed, preparation-deferred, process launch failed before running, cancelled, skipped/abandoned, parked/waiting, failed, message-only, reconciliation-only, or a stale prior generation. A manually requested diagnostic of a retained failed candidate is allowed only as an explicitly named advisory product; it is not automatic acceptance evaluation and cannot satisfy a required gate.

The policy may be snapshotted when the generation starts, but the route and record are materialized only at the predicate above. This avoids credential checks and route pins for work that never reaches evaluation.

### 4.4 State machine

```text
PreparingBundle -> Queued -> Running -> EvidenceAvailable -> Consumed
                       |         |  \
                       |         |   -> RetryBackoff -> Queued
                       |         -> TimedOut | Malformed | Unavailable
                       -> Cancelled
```

`PreparingBundle`, `Queued`, `Running`, `RetryBackoff`, and infrastructure terminal states are evaluation facts only. `EvidenceAvailable` contains an immutable verdict reference. `Consumed` means the acceptance controller performed a single fenced policy decision; it does not mean the verdict was “pass.”

For advisory evaluation the source may reach terminal `Done` while the record remains queued/running/unavailable. For a required evaluation the source remains `Finalizing/AwaitingAcceptance`; it is never changed to `Open` and its source attempt is never restarted by this state machine.

## 5. Evidence bundle

### 5.1 Canonical schema

All maps use sorted keys and all arrays whose order is not semantic are sorted before canonical UTF-8 JSON serialization. Each payload is hashed first; the manifest containing those CIDs is then hashed. Timestamps are provenance, not semantic identity. No absolute host path or secret is included.

```json
{
  "schema": 1,
  "product": "bounded-evaluation",
  "source": {
    "task_id": "compile-index",
    "generation": 2,
    "source_attempt_id": "attempt-2-1",
    "source_fence": 7,
    "finalization_round": 1,
    "candidate_digest": "b3:candidate...",
    "candidate_manifest_digest": "b3:manifest...",
    "dependency_revision_digest": "b3:deps..."
  },
  "policy": {
    "digest": "b3:policy...",
    "applicability": "advisory",
    "validation_criteria_digest": "b3:criteria..."
  },
  "route": {
    "adapter": "pi-rpc-v1",
    "exact_route": "pi:openai-codex:gpt-5.6-sol",
    "reasoning": "low",
    "digest": "b3:route..."
  },
  "items": [
    {
      "kind": "task-contract",
      "cid": "b3:task...",
      "media_type": "application/json",
      "bytes": 1384,
      "provenance": {"event_id": "ev_task", "trust": "local-authoritative"},
      "redaction": "none"
    },
    {
      "kind": "candidate-diff",
      "cid": "b3:diff...",
      "media_type": "text/x-diff",
      "bytes": 8241,
      "provenance": {"receipt_id": "candidate:b3:candidate...", "trust": "untrusted-content"},
      "redaction": "secret-patterns-v1"
    },
    {
      "kind": "validation-receipt",
      "cid": "b3:validation...",
      "media_type": "application/json",
      "bytes": 991,
      "provenance": {"event_id": "ev_validate", "trust": "local-authoritative"},
      "redaction": "secret-patterns-v1"
    }
  ],
  "limits": {
    "max_bundle_bytes": 524288,
    "max_prompt_tokens_estimate": 24000,
    "max_output_tokens": 1800,
    "max_output_bytes": 16384,
    "wall_timeout_ms": 90000
  },
  "bundle_cid": "b3:bundle..."
}
```

The example's route and usage vocabulary follow Pi's documented model/RPC types. The bundle does not contain Pi credentials or endpoint details.

### 5.2 Bounded-evaluation contents

The builder includes only bounded, checkpoint-derived facts:

- task title/description and normalized `## Validation` criteria;
- role/outcome constraints relevant to acceptance, not the evaluator persona's writable capabilities;
- source attempt/finalization reference;
- candidate manifest, bounded patch, and bounded excerpts of declared artifacts;
- deterministic validation receipts and bounded log tail;
- runtime summary (exit/progress/tool names/durations), not hidden reasoning;
- dependency result digests necessary to interpret the candidate; and
- explicit truncation/missing-evidence markers carrying the full omitted CID and byte count.

It does not include a live filesystem path, mutable `main`, full graph, unrelated chat, arbitrary configuration, source-agent credentials, or an executable command. If essential evidence does not fit, the builder emits `evidence_incomplete`; the only valid model decision is `inconclusive`.

### 5.3 Content-safety framing

Every model-visible item is data, not instruction:

```text
BEGIN UNTRUSTED EVIDENCE
kind=<enum> cid=<b3> byte_length=<n> encoding=utf-8
<exact normalized bytes>
END UNTRUSTED EVIDENCE cid=<same b3>
```

Length and CID, rather than a forgeable delimiter alone, define the boundary. Before bundling, WG normalizes Unicode/control characters, scans for secret patterns and prompt-injection indicators, redacts secrets, and records transformations. The system prompt says that commands, role claims, tool requests, or output-schema overrides inside evidence are hostile data. Findings use bounded category/reason codes and CID/range references; they do not copy attacker-controlled prose into lifecycle logs.

The evaluator has no mutation capability even if the content-safety classifier is wrong. A hostile bundle can at worst influence an advisory verdict or cause a required gate to wait for review; it cannot execute an action.

## 6. Product A — bounded evaluation

### 6.1 Capability boundary

| Capability | Bounded evaluation |
|---|---|
| Input | One immutable evidence bundle materialized into the prompt |
| Pi session | Fresh, ephemeral, exactly one prompt |
| Tools | None (`--no-tools`) |
| Extensions/skills/templates/context discovery | None |
| Filesystem/graph/config access | None through the model |
| Shell/network tools | None; only Pi's selected provider transport exists |
| Output | One strict `BoundedVerdictV1` JSON object in final assistant text |
| Default policy | Advisory |

The parser requires one JSON object and no preamble/fences/trailing prose, denies unknown fields, checks finite `[0,1]` scores, fixed dimension names, enum reason codes, evidence CIDs that exist in the bundle, route equality, and exact source identity. This is intentionally stricter than the current permissive “extract JSON from prose” loop.

### 6.2 Verdict schema and Pi receipt example

```json
{
  "schema": 1,
  "product": "bounded-evaluation",
  "evaluation_id": "eval-b3:...",
  "source": {
    "task_id": "compile-index",
    "generation": 2,
    "source_attempt_id": "attempt-2-1",
    "finalization_round": 1,
    "candidate_digest": "b3:candidate..."
  },
  "bundle_cid": "b3:bundle...",
  "policy_digest": "b3:policy...",
  "route_digest": "b3:route...",
  "decision": "accept",
  "score": 0.91,
  "confidence": 0.82,
  "dimensions": {
    "correctness": 0.93,
    "completeness": 0.88,
    "validation_support": 0.94,
    "constraint_fidelity": 0.89
  },
  "criteria": [
    {
      "criterion_id": "validation-1",
      "outcome": "satisfied",
      "reason_code": "VALIDATION_RECEIPT_PASS",
      "evidence_refs": ["b3:validation..."]
    }
  ],
  "findings": [],
  "missing_evidence": []
}
```

WG wraps the model document in a non-model-authored invocation receipt derived from Pi RPC events:

```json
{
  "adapter": "pi-rpc-v1",
  "pi_version": "0.82.0",
  "reported_provider": "openai-codex",
  "reported_model": "gpt-5.6-sol",
  "thinking_level": "low",
  "stop_reason": "stop",
  "settled": true,
  "usage": {
    "input": 10840,
    "output": 612,
    "cacheRead": 0,
    "cacheWrite": 0,
    "totalTokens": 11452,
    "cost": {"input": 0.0, "output": 0.0, "cacheRead": 0.0, "cacheWrite": 0.0, "total": 0.0}
  },
  "tool_calls_seen": 0
}
```

Those names and stop reasons come directly from Pi's documented `AssistantMessage`/usage schema. The wrapper, not the model, supplies them.

### 6.3 Budget enforcement

Pi RPC 0.82.0 does not expose a per-prompt `maxTokens` setter. The first implementation is therefore explicit about its enforceable bounds:

- bundle byte and estimated-input-token ceilings are enforced before spawn;
- one no-tools prompt means no model/tool loop;
- the adapter counts streamed text bytes and sends `abort` at `max_output_bytes`;
- it checks provider-reported output/total tokens at every completed turn and rejects an over-budget result;
- an external wall deadline kills and reaps the process; and
- no automatic Pi retry runs inside a runner attempt.

A later SDK adapter may additionally clamp the selected model's `maxTokens`; that is an optimization, not a weakening of the byte/deadline guards. Budget exceeded produces infrastructure evidence, never a partial semantic verdict.

## 7. Product B — deep-readonly FLIP

### 7.1 What makes this genuine FLIP

Deep-readonly FLIP is not “bounded evaluation with a longer prompt.” A valid FLIP record requires all of the following:

1. **Blind latent-intent phase:** from candidate artifacts/diff, validation, runtime behavior, and dependency outputs—but without the original task wording/conversation—the evaluator commits a structured hypothesis of the intended goal, constraints, invariants, and likely failure modes.
2. **Intent reveal and comparison:** a fresh Pi call receives the immutable hypothesis, then may read the original task intent/conversation and compare them. The original intent cannot retroactively alter the stored hypothesis.
3. **System comprehension:** it inspects at least the source, one relevant dependency or explicit “none,” the validation/runtime path, and the redacted effective configuration that could change behavior. Each claim cites evidence.
4. **Counterfactual probing:** it answers concrete “what would change if…” questions covering a changed dependency, a configuration/route variation, and one failure/edge condition. Counterfactuals are reasoned against code/artifact/trace evidence, not merely restated requirements.
5. **Cross-component analysis:** it checks interface assumptions between the candidate and dependencies/consumers, configuration and runtime, and validation and claimed outcome.
6. **Fidelity decision:** it distinguishes latent-intent alignment from ordinary correctness and reports uncertainty/missing evidence.

If any required phase or evidence class is absent, the schema requires `inconclusive`; a shallow summary cannot be relabeled FLIP.

### 7.2 Two isolated phases

**Phase I — latent hypothesis.** Active tools expose candidate/artifact/diff/validation/trace/dependency-output observations but not task description, originating conversation, or intent. The model finishes with `submit_latent_hypothesis`, a terminating in-memory tool modeled on Pi's documented `terminate: true` structured-output example. The canonical hypothesis is saved to CAS.

**Phase II — comparison and counterfactual.** A fresh ephemeral Pi process receives the phase-I CID and can use the complete allowlist below, including intent. It finishes with `submit_flip_verdict`. Starting a fresh process prevents session branches, compaction, or phase-II context from contaminating phase I.

Both phase route snapshots are exact and usually identical. If policy intentionally chooses two routes, both are persisted before either phase starts. Failure never chooses a replacement route.

### 7.3 Observation-only allowlist

| Tool | Reads | Explicit denials |
|---|---|---|
| `observe_candidate_manifest` | checkpoint manifest by exact candidate CID | no path traversal or mutable checkout |
| `observe_artifact` | declared artifact by item CID and bounded range | no arbitrary file path |
| `observe_diff` | checkpoint-derived patch hunks | no Git command |
| `observe_graph_slice` | source/dependency/consumer projection at pinned graph revision | no graph write, claim, message, retry, or hidden task creation |
| `observe_original_intent` | task text and provenance-linked conversation excerpts; phase II only | no session continuation or author impersonation |
| `observe_dependency_context` | dependency contracts/result digests and selected artifacts | no recursive unbounded graph walk |
| `observe_validation` | validation specs/receipts/log ranges | no command execution |
| `observe_runtime_trace` | filtered event/tool/exit/timing records | no PID/process control; no hidden reasoning |
| `observe_effective_config` | redacted resolved route, policy, feature flags, limits, config provenance | no secrets, raw auth, environment dump, or config mutation |
| `search_evidence` | bounded local index over the bundle CIDs | no web/network search |
| `submit_latent_hypothesis` / `submit_flip_verdict` | validates and returns an in-memory structured result | no environmental mutation |

The extension registers no `bash`, `write`, `edit`, user-message, provider-registration, session-switch, or generic `read` tool. Every observation request is `(evaluation_id, bundle_cid, enum kind, CID/range)` and is served by a read-only broker. The broker rejects unknown IDs, absolute paths, `..`, symlink escape, over-budget recursion, and a source tuple mismatch.

### 7.4 Containment

Pi's security documentation says extensions have the process's full permissions and Pi has no built-in sandbox. Therefore deep FLIP additionally requires:

- an empty working directory;
- a read-only mount or broker containing only the requested evidence CIDs;
- no source checkout, graph file, config file, agent session, SSH directory, cloud config, or general home mount;
- a private writable temp directory discarded after the run;
- child-process execution denied; and
- no model-callable network capability. The only network activity is Pi's own already-selected provider transport; the observation extension contains no fetch/socket/provider registration code.

The exact extension bytes and compatibility version are embedded/versioned like `pi-worksgood`. Startup verifies the digest. Unsupported containment is a loud `deep_flip_containment_unavailable`, not permission-filtered host execution.

### 7.5 FLIP verdict

```json
{
  "schema": 1,
  "product": "deep-readonly-flip",
  "evaluation_id": "flip-b3:...",
  "source": {
    "task_id": "compile-index",
    "generation": 2,
    "source_attempt_id": "attempt-2-1",
    "finalization_round": 1,
    "candidate_digest": "b3:candidate..."
  },
  "bundle_cid": "b3:deep-bundle...",
  "latent_hypothesis_cid": "b3:hypothesis...",
  "policy_digest": "b3:policy...",
  "route_digests": ["b3:phase1-route...", "b3:phase2-route..."],
  "fidelity": "aligned",
  "score": 0.86,
  "confidence": 0.76,
  "latent_intent": {
    "goal_code": "INDEX_BUILD_IS_DETERMINISTIC",
    "constraint_codes": ["NO_STALE_DEPENDENCY", "ATOMIC_REPLACE"],
    "evidence_refs": ["b3:artifact...", "b3:trace..."]
  },
  "counterfactuals": [
    {
      "id": "dependency-stale",
      "question_code": "DEPENDENCY_REVISION_CHANGES",
      "expected_effect_code": "REVALIDATE_OR_REJECT",
      "observed_support": "supported",
      "evidence_refs": ["b3:deps...", "b3:diff..."]
    },
    {
      "id": "config-route",
      "question_code": "EFFECTIVE_CONFIG_CHANGES",
      "expected_effect_code": "OUTPUT_SEMANTICS_UNCHANGED",
      "observed_support": "partial",
      "evidence_refs": ["b3:config..."]
    },
    {
      "id": "interrupted-write",
      "question_code": "FAILURE_DURING_PUBLISH",
      "expected_effect_code": "OLD_INDEX_REMAINS_READABLE",
      "observed_support": "supported",
      "evidence_refs": ["b3:validation..."]
    }
  ],
  "cross_component_findings": [
    {
      "category": "DEPENDENCY_CONTRACT",
      "severity": "low",
      "reason_code": "REVISION_DIGEST_CHECKED",
      "evidence_refs": ["b3:deps...", "b3:trace..."]
    }
  ],
  "missing_evidence": []
}
```

## 8. Executor-neutral interface and route safety

```rust
trait EvaluationExecutorAdapter {
    fn adapter_id(&self) -> &'static str;
    fn capabilities(&self) -> EvaluationCapabilities;
    fn probe_exact_route(&self, route: &EvaluationRouteSnapshot) -> Result<RouteReceipt>;
    fn invoke(
        &self,
        request: &EvaluationInvocation,
        sink: &mut dyn EvaluationEventSink,
    ) -> Result<InvocationReceipt, EvaluationExecutionError>;
}

struct EvaluationInvocation {
    evaluation_id: String,
    runner_attempt_id: String,
    product: EvaluationProduct,
    source: SourceCandidateRef,
    bundle_cid: String,
    exact_route: EvaluationRouteSnapshot,
    capability_manifest: CapabilityManifest,
    budget: EvaluationBudget,
}
```

The interface knows `PiRpcV1`, `CodexCliEvaluationV1`, and `ClaudeCliEvaluationV1` adapter identities. Pi is recommended/default. Codex/Claude adapters are used only by explicit evaluation configuration and must produce the same domain receipt. This does not change worker/chat routing.

Rules:

1. `exact_route.adapter`, handler, provider/model, reasoning, adapter version, and capability manifest are hashed and persisted before queueing.
2. The queue dispatches by `adapter`, never by parsing a failure or consulting the current profile.
3. A Pi route failure returns a Pi failure. It cannot call Codex/Claude/Nex. The inverse is also true.
4. No implicit model fallback runs. A retry repeats the same exact route.
5. Operator `wg evaluate reroute <id> --route ...` is an audited action that creates a new `route_generation`, new runner-attempt IDs, and a new route digest. It never rewrites old receipts. Required-gate policy may forbid rerouting.
6. Reported provider/model must equal the persisted route. Mismatch is `route_drift`, even if the JSON verdict looks valid.
7. The existing direct Codex worker/chat paths (`src/dispatch/plan.rs`, `src/commands/spawn/execution.rs`, interactive chat handling) are untouched. Evaluation tests may invoke a fake explicit Codex adapter, but this design does not broaden which normal tasks may run on Codex or Claude.

**Native Codex retention is a release-blocking non-regression:** all existing explicit Codex worker and live-chat scenarios must remain byte-for-byte equivalent in route selection and launch topology when evaluation is disabled or Pi is unavailable.

## 9. Dedicated agency queue

### 9.1 Scheduling class

`AgencyEvaluation` is a new admission class, not a `BuildClass` value and not a normal task:

- independent FIFO/priority queue over `EvaluationRecord`s;
- independent `evaluation.max_concurrency` (initial default `1` when enabled);
- independent process leases recorded inside `runner_attempts` and a small agency-runner registry;
- no worktree, branch, target directory, build-cache lease, normal agent registry entry, assignment, worker slot, or `max_build_agents` check;
- one bounded process per runner attempt;
- admission based only on agency concurrency, exact adapter availability, record backoff, and shutdown; and
- queue work happens after lifecycle reconciliation and independently of worker dispatch. A full worker pool does not stop the agency lane, and a full agency lane does not stop workers.

A scheduler denial leaves the record `Queued` with `next_eligible_at`; it creates no runner attempt and charges no failure.

### 9.2 Budgets and retry

Initial policy:

| Product | Automatic attempts | Backoff | Wall deadline | Prompt estimate | Output |
|---|---:|---|---:|---:|---:|
| bounded | 2 | 2s, then terminal | 90s | 24k tokens | 1,800 reported tokens / 16 KiB stream |
| deep phase | 2 per phase | 5s, then terminal | 180s | 64k tokens | 4,000 reported tokens / 48 KiB stream |

The budget is snapshotted per record. Timeout, process exit, credential/model unavailable, RPC framing error, malformed output, route drift, and containment failure are runner outcomes. They never use source retry/spawn counters or provider-health counters for normal workers.

Retry classification:

- transient provider/transport error, timeout, or malformed model JSON: at most one automatic retry on the exact route;
- unavailable executable/credential/model, route drift, schema incompatibility, bundle corruption, or containment failure: no hot retry; wait for operator/config change;
- semantic `reject`/`accept`/`inconclusive`: never retried automatically merely to seek a different score.

The CLI exposes `wg evaluate retry <evaluation-id>` and `reroute` separately. Retry reuses the route; reroute is explicit provenance.

## 10. Exactly-once serialization

The serialization order is normative:

1. The source finalizer durably checkpoints candidate and manifest CAS objects.
2. Under `graph.lock`, it verifies the creation predicate and appends exactly one `EvaluationRecord(PreparingBundle)` plus an outbox action. Advisory source terminalization may occur in this same lifecycle commit; required source remains finalizing.
3. The bundle assembler reads only event/CAS references from the action, writes every item with create-if-absent + fsync, writes the canonical manifest last, and returns `bundle_cid`.
4. Under `graph.lock`, a CAS links that CID and moves the record to `Queued`. A different CID for the same request is corruption.
5. Under `graph.lock`, the agency scheduler acquires one agency lease and appends `EvaluationRunnerAttempt(Running)` with an idempotency key. Only then does it spawn Pi.
6. Raw protocol bytes go to a temporary file. On settlement WG writes a filtered invocation receipt to CAS; hidden reasoning and secrets are not persisted.
7. WG strict-parses the model result, constructs the full verdict + Pi receipt, computes `verdict_id`, and writes `<verdict-id>.json` with create-new + fsync. Existing identical bytes are a replay; different bytes are a conflict.
8. Under `graph.lock`, WG links the verdict only if the exact source, bundle, policy, route, runner attempt, and current record still match; it terminalizes the runner attempt and releases the agency lease atomically.
9. The acceptance controller performs `consumed_verdict_id: None -> Some(id)` under the source generation/attempt/fence CAS. Repeating the same ID is a no-op. A different ID after consumption is retained as later evidence and cannot change the decision.
10. Only the acceptance controller requests the source lifecycle outcome. Evaluation runners and linkers have no source-transition capability.

Crash recovery resumes the first incomplete numbered step. It never re-invokes a model after a valid verdict file exists, never consumes twice, and never evaluates a substituted checkout.

## 11. Advisory, hard-gate, and failure semantics

### 11.1 Advisory default

- source acceptance ignores availability and score;
- source may be terminal before the model runs;
- `accept`, `reject`, `inconclusive`, timeout, malformed, and unavailable all remain visible candidate-bound evidence;
- a low/late verdict may recommend an explicit new task or retry, but cannot create one; and
- Pi unavailable means `Done + evaluation: unavailable(adapter=pi, route=...)`, not fallback and not missing output.

### 11.2 Selective hard gates

A hard gate must be selected on the task/generation before completion and pass rollout checks in §14. It is never inferred from tags or evaluator output.

- valid `accept` satisfying threshold/quorum permits the finalizer to continue;
- valid semantic `reject` supplies candidate rejection evidence to the finalizer;
- `inconclusive`, malformed, unavailable, timeout, or missing evidence leaves the source explicitly `Finalizing/AwaitingAcceptance` with an operator action;
- infrastructure failure never becomes semantic rejection;
- no result changes the source to `Open`; and
- retry requires evaluation retry or, after a terminal rejection, the ordinary generation/retry controller.

A policy-controlled audited waiver is acceptance evidence. It is never a silent promotion.

### 11.3 Visibility

`wg show TASK` displays, without requiring `--internal`:

```text
Evaluation: bounded advisory — unavailable
  id: eval-b3:...  candidate: b3:candidate...  attempt: attempt-2-1
  route: pi:openai-codex:gpt-5.6-sol (pinned, no fallback)
  last attempt: timeout at 90s  next: wg evaluate retry eval-b3:...
```

For a hard gate:

```text
Acceptance: waiting on required bounded evaluation
  source is Finalizing; implementation will not be reopened
```

`wg evaluate status [TASK|ID] --json` exposes queue position, state, budgets, runner attempts, bundle/verdict CIDs, usage/cost, exact adapter/route, retry time, and gate effect. `wg status`, service status, TUI/Viz inspector, and daemon logs show separate worker/build/agency occupancy. Virtual satellite labels are display-only and marked `virtual evaluation record`.

## 12. Configuration and migration

### 12.1 New configuration

```toml
[evaluation]
rollout_stage = "disabled"       # disabled|shadow|advisory-canary|advisory
max_concurrency = 1
default_policy = "none"         # none|advisory; "required" rejected globally
executor = "pi"
route = "pi:openai-codex:gpt-5.6-sol"
reasoning = "low"

[evaluation.bounded]
timeout_secs = 90
max_bundle_bytes = 524288
max_prompt_tokens = 24000
max_output_tokens = 1800
max_output_bytes = 16384
max_attempts = 2

[evaluation.deep_readonly_flip]
enabled = false                  # explicit/selective only
max_concurrency = 1
timeout_secs_per_phase = 180
```

Role-specific exact executor/route remains possible. An explicit `codex:` route selects only the Codex evaluation adapter; it does not modify worker/chat defaults.

### 12.2 Legacy data

- Existing `.evaluate-*`/`.flip-*` rows remain readable and drain on the legacy path during the transition. New-plane code never retries them as normal records.
- At new-plane cutover, unclaimed legacy satellites without verdicts are cancelled/archived as legacy scaffolding. They are not converted for sources lacking an exact candidate checkpoint.
- A legacy verdict with exact source attempt **and** candidate binding may import as immutable evidence. Existing verdicts without candidate digest are historical/advisory only and cannot satisfy a new required gate.
- Legacy `PendingEval` with required semantics remains on its imported hold until safely drained or explicitly migrated with a pinned candidate; no guessed pass.
- `auto_evaluate=true` is migration input for the legacy path only. It does not silently advance `evaluation.rollout_stage` or enable the new advisory default.
- `Task.evaluation_records` is serde-defaulted. Old graphs remain readable.

No historical file is rewritten in place. Migration appends provenance and is idempotent.

## 13. File-level implementation seams and ownership

| File/module | Change ownership |
|---|---|
| `src/evaluation/mod.rs` (new) | Public schemas, IDs, states, executor-neutral trait, error taxonomy. |
| `src/evaluation/bundle.rs` (new) | Canonicalization, redaction/content-safety framing, CAS writes, limits. |
| `src/evaluation/store.rs` (new) | Create-once verdict/receipt storage and integrity verification. |
| `src/evaluation/policy.rs` (new) | Lazy creation predicate, advisory/required decision, rollout certificate checks. |
| `src/evaluation/queue.rs` (new) | Dedicated agency leases, concurrency, backoff, recovery; no worker/build APIs. |
| `src/evaluation/pi_rpc.rs` (new) | Strict LF RPC client, state/route verification, event/usage parsing, budgets, reaping. Do not reuse permissive JSON extraction. |
| `src/evaluation/flip.rs` (new) | Phase separation, minimum evidence/counterfactual rules, observation capability manifest. |
| `src/evaluation/adapters/{codex,claude}.rs` (optional adapters) | Explicit adapter implementations only; no dispatch changes. |
| `pi-evaluation/src/deep-readonly.ts` + committed embedded build (new) | Exact observation and terminating-output tools; no provider registration, shell, or generic read. Re-embed/check for drift like `worksgood-pi`. |
| `src/graph.rs` | Add serde-defaulted record projection to `Task`; no new task status or edges. |
| `src/lifecycle.rs` | Typed create/link/consume requests and actor authorization; evaluator may append evidence only. |
| `src/commands/done.rs` / finalization controller | Invoke the lazy policy after exact checkpoint; never call a model while holding `graph.lock`. |
| `src/commands/eval_scaffold.rs` | Stop creating new evaluation/FLIP rows; retain assignment and bounded legacy migration helpers temporarily. |
| `src/commands/service/coordinator.rs` | Remove new eager catch-all and inline eval dispatch; tick the agency queue independently after reconciliation, not inside `slots_available`. |
| `src/commands/evaluate.rs` | Become create/status/retry/reroute/deep-request CLI over records. Move legacy evaluator into an explicitly named compatibility module until removed. |
| `src/service/llm.rs` | Leave current lightweight callers for non-evaluation compatibility; new plane uses the adapter trait. No cross-executor fallback. |
| `src/stream_event.rs` | Continue worker accounting unchanged. The evaluation adapter has a strict parser because current translation intentionally skips malformed lines. |
| `src/config.rs`, `src/config_defaults.rs`, `src/config_migrate.rs` | New rollout/policy/lane/budget fields, global-required rejection, legacy mapping. |
| `src/commands/show.rs`, `status.rs`, service IPC | Record/queue visibility and separate occupancy. |
| `src/tui/viz_viewer/{state,render,event}.rs`, `src/commands/viz/*` | Candidate/verdict pane and virtual-record rendering; no task-edge semantics. |
| `src/commands/pi_handler.rs`, `src/commands/spawn/execution.rs`, `src/dispatch/*`, normal chat code | **No behavior change** for native Pi/Codex/Claude workers or chat. |
| `tests/fixtures/fake-pi-evaluation/*` | Credential-free RPC fixtures. |
| `tests/smoke/scenarios/*` + grow-only manifest | Permanent source/terminal/TUI flows below. |

Serialization-bearing work must land in this order: record schemas/defaults → lazy lifecycle events → CAS/bundle → Pi adapter → agency queue → CLI/visibility → stop eager creation → migration → canary enablement. Coordinator/lifecycle writers must be serialized with the authoritative-lifecycle work; queue/adapter work may proceed in parallel only after schemas are fixed.

## 14. Rollout safety

Rollout is a one-way, recorded state machine:

1. **Disabled (default):** schemas, status, and migration diagnostics only. No bundle and no model call.
2. **Shadow:** eligible candidate detection and deterministic bundle construction; no model call and no gate.
3. **Advisory canary:** explicitly named tasks call Fake Pi, then live Pi where credentials exist. Source outcome is unaffected.
4. **Recorded canary success:** WG writes a content-addressed certificate containing scenario versions, Pi version/route, counts, malformed/timeout/unavailable observations, route-integrity results, human-flow evidence, and operator acknowledgment.
5. **Advisory:** bounded evaluation may become the optional default.
6. **Selective required:** only an explicit per-task/generation policy may hard-gate, and only when the certificate meets the compiled minimum. Deep FLIP remains explicit/selective.

Structural guards:

- `default_policy="required"` is rejected by config validation in this release, regardless of certificate.
- a compiled `GLOBAL_EVALUATION_HARD_GATE_SUPPORTED = false` prevents a global hard gate;
- stage skipping is rejected (`advisory` requires a valid certificate); expired/incompatible certificates fall back to disabled/advisory, never hard;
- global FLIP is not a configuration value in this batch; and
- canary failure cannot alter source state.

A later release may deliberately remove the global-hard-gate guard only with a new design/release decision and recorded production canaries.

## 15. Validation plan

### 15.1 Credential-free Fake Pi

`tests/fixtures/fake-pi-evaluation/pi` implements strict RPC, records argv/stdin, and requires all provider credentials to be unset. Fixtures emit documented Pi shapes, including `get_state`, correlated prompt response, assistant message, `turn_end.message.usage`, `agent_end`, `agent_settled`, and stats.

| Fixture | Required assertion |
|---|---|
| `success` | Valid bounded verdict and Pi usage become one evidence file and one consumption. No tools/resources/session. |
| `malformed-output` | Final text is not schema-valid; one exact-route retry, then visible `Malformed`; no partial verdict/source change. |
| `timeout` | Fake accepts then hangs; WG aborts/kills/reaps at deadline; record is `TimedOut`; source semantics follow advisory/required policy. |
| `route-drift` | `get_state` or assistant reports a different provider/model; adapter rejects before consumption. No ambient route is tried. |
| `duplicate-delivery` | Same settled/result bytes are replayed across a simulated crash; one verdict link and one consume. Conflicting bytes quarantine. |
| `unavailable-executor` | Missing/nonzero fake Pi produces `Unavailable(adapter=pi)` and proves fake `codex`/`claude` sentinels were never invoked. |

Also cover malformed LF frames, `stopReason=length|error|aborted`, tool events in bounded mode, usage repetition, budget overflow, stale generation/candidate, config drift, operator reroute provenance, and deep-tool path/ID escape attempts.

### 15.2 Named permanent scenarios

1. **`pi_eval_lazy_candidate_only`** — publish/open/status/daemon ticks create no records; a real Fake-Pi worker completes and checkpoints; exactly one selected record then appears.
2. **`pi_eval_never_ran_matrix`** — never attempted, never spawned, admission-deferred, preparation/launch failed, cancelled, skipped/abandoned, open, message-only, and reconciliation-only cases create zero work.
3. **`pi_eval_no_zombie_after_retry`** — attempt N evidence cannot schedule or satisfy N+1; no graph satellite exists.
4. **`pi_eval_admission_deferral_neutral`** — more than breaker threshold ticks charge no source/agency attempt and create no evaluation until the source truly runs.
5. **`pi_eval_attempt_candidate_route_fence`** — route/config changes and deliberately different main content cannot alter the exact candidate/attempt/route.
6. **`pi_eval_agency_lane_independence`** — fill every worker and build-heavy slot; bounded evaluation still runs in its one agency slot; fill agency slot and a worker still starts.
7. **`pi_eval_timeout_budget_visible`** — deadline and token/byte guards terminate, reap, and render exact diagnostics.
8. **`pi_eval_duplicate_delivery_exact_once`** — restart at bundle, claim, verdict write, link, and consume boundaries.
9. **`pi_eval_no_cross_executor_fallback`** — Pi unavailable never executes Codex/Claude; explicit fake Codex evaluation unavailable never executes Pi/Claude.
10. **`pi_eval_bounded_vs_deep_capabilities`** — bounded has zero tools; deep exposes only named tools; phase I cannot observe intent; all mutation/path/network probes fail.
11. **`pi_eval_deep_flip_is_not_summary`** — missing blind hypothesis, three counterfactual classes, or cross-component evidence forces `inconclusive`.
12. **`pi_eval_rollout_guard`** — fresh config disabled; stage skipping/global required/global FLIP rejected; selective required refused until a valid canary certificate.
13. **`pi_eval_native_codex_retention`** — existing explicit Codex worker and live-chat launch/route tests pass with evaluation disabled and Pi absent.

### 15.3 Actual terminal and TUI human flow

Library calls are insufficient. Two smoke scenarios drive the installed binary and user surfaces.

**Terminal flow (`pi_eval_terminal_completion_flow.sh`):**

1. start a real WG service with isolated HOME/PATH and Fake Pi;
2. create/publish a source with advisory bounded evaluation selected;
3. let a real fake worker claim, produce an artifact, call `wg done`, quiesce, and checkpoint;
4. observe `wg show SOURCE` transition from candidate completion to terminal `Done` with `evaluation: queued/running` and then a visible verdict/CID/usage;
5. run `wg evaluate status SOURCE --json` and prove exact attempt/candidate/route;
6. repeat with Pi unavailable and prove source stays terminal with visible `Unavailable`, no fallback;
7. repeat with an explicit required gate after installing a canary certificate: source visibly waits in finalization, restoring Fake Pi completes the **same** record, and the source finishes without reopening.

**TUI/Viz flow (`pi_eval_tui_verdict_flow.sh`):**

1. launch `wg tui` in a private tmux/PTY and start the same source through the real keymap/command UI;
2. drive keys to the task inspector while the worker completes;
3. assert `wg tui-dump`/pane output shows the source terminal/advisory badge, separate agency-lane activity, pinned Pi route, candidate CID, then decision/score/usage;
4. request deep-readonly FLIP through the actual inspector action/command, observe phase-I and phase-II progress plus observation-tool names, then inspect counterfactual/cross-component evidence;
5. compare graph/source/config hashes before and after FLIP and assert no mutation other than evaluation-record/evidence projections;
6. repeat the unavailable hard-gate view and assert it says “waiting on evaluation; source will not be reopened,” with the retry command.

The scenario must use tmux keystrokes/PTY and rendered output, not invoke the render function directly. It and the terminal scenario are added to the grow-only smoke manifest with permanent ownership.

### 15.4 Build/test gate

Implementation tasks require RED-first unit/property tests, the Fake-Pi matrix, the two human flows, permanent smoke ownership, `cargo fmt --check`, `cargo clippy`, `cargo build --locked`, and the full test suite. Pi-live validation is an additional canary and may loud-SKIP only when the selected Pi route lacks credentials; credential-free fixtures may never skip.

## 16. Acceptance and non-regression checklist

The plane is ready only when:

- every failure in §2 has its named passing regression;
- no evaluation record exists without the exact running/completed candidate predicate;
- bounded and deep capability manifests are disjoint and enforced;
- deep FLIP cannot pass without latent reconstruction, intent comparison, counterfactuals, and cross-component evidence;
- queue occupancy is independent of worker/build occupancy;
- Pi protocol errors and usage are visible and exact;
- identical evidence consumes once across every crash boundary;
- no adapter or route crosses implicitly;
- advisory unavailability leaves source terminal and visible;
- required unavailability waits without reopening;
- rollout starts disabled and global hard gating remains structurally impossible;
- terminal/TUI humans can follow source completion to verdict/FLIP evidence; and
- native Codex worker/chat scenarios remain unchanged without broadening direct Codex/Claude task scope.

## 17. Rationale and rejected alternatives

**Keep safer eager satellites.** Rejected. Ordinary task identity, edges, worker slots, retries, and statuses are the wrong domain even if guarded better.

**Use the current FLIP prompt but call it deep.** Rejected. A two-summary comparison without blind evidence, effective-system comprehension, counterfactuals, and cross-component inspection does not test latent intent.

**Give FLIP `read` and `bash` and promise not to write.** Rejected. Pi documents that project trust is not a sandbox and extensions/tools run with user permissions. Generic bash also violates the network/process boundary.

**Use bounded evaluation with the structured-output extension.** Rejected for the default. That would no longer be no-tools. Strict final-text JSON is honest about Pi 0.82.0's public RPC contract; malformed output is handled as evidence failure.

**Fall back from Pi to Codex/Claude when Pi is down.** Rejected. The verdict would have a different producer than the attempt-bound policy selected. Operators can explicitly reroute with audit.

**Make every evaluation a hard gate after canaries.** Rejected. Routine grading is probabilistic and infrastructure availability is not source quality. Advisory is the safe default; high-risk hard gates are explicit and candidate-bound.

**Move everything to a new database first.** Rejected. A serde-defaulted source projection plus content-addressed immutable objects is the smallest safe implementation and migrates cleanly to the authoritative ledger later.

The central rule is:

> Evaluation may observe one immutable candidate and append attributable evidence. It may never become source execution, source identity, or source lifecycle authority.
