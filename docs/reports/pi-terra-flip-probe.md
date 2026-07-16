# Pi Terra probe for FLIP and agency routing

**Task:** `probe-pi-terra`  
**Probe date:** 2026-07-13 UTC  
**Decision:** Use Terra as the default Pi evaluator/reviewer/FLIP tier, after the separate WG lifecycle fix is landed. Keep Luna for bounded assignment/triage work and Sol for strong work and escalation.

## Executive result

All three OpenAI-Codex models were available through the existing Pi OAuth login. In a bounded direct matrix, all 24 calls (2 trials × 4 prompt classes × 3 models) exited 0, contained assistant text, emitted valid Pi v3 NDJSON, returned the requested JSON schema, and reported usage. All six assignment trials selected the programmer candidate. Each model also completed the real two-phase `wg evaluate run synth --flip` path in an isolated graph.

Terra is the best default for FLIP/evaluation in this three-model system: it passed every probe, was slightly faster than Luna and Sol in this small sample, cost less than half of Sol, and is a more appropriate capability tier than Luna for semantic reconstruction and scoring. This is a routing recommendation, not a claim that Terra fixes transport or lifecycle defects.

The recent incident had at least three independent failure classes:

1. **Intermittent empty Pi one-shot:** historical Luna FLIP calls reached WG's post-Pi translation check with no final text. The prior call did not preserve raw NDJSON, so the exact split between an empty upstream response, Pi emission, and a one-off event-shape/parser mismatch remains unresolved. Current direct and WG probes did not reproduce it.
2. **Node `output-guard.js` EPIPE:** reproduced by closing Pi's stdout consumer early. This is output transport/backpressure behavior, not model capability.
3. **`FailedPendingEval` respawn storm:** a successful FLIP child could be scheduled over a `FailedPendingEval` parent, but `wg done` would not allow that child to complete. The durable evaluation was repeatedly regenerated. This is a WG state-machine bug, not something any model can fix.

## Environment and availability

| Component | Observed value |
|---|---|
| Pi | `0.80.6` at `/home/bot/.nvm/versions/node/v25.4.0/bin/pi` |
| WG binary | `wg 0.1.0` at `/home/bot/.cargo/bin/wg` |
| Probe checkout | `ca311b09619f9c93ba078f929ab394cd57665f06` |
| WG binary freshness | built 2026-07-13 10:30 UTC; `wg dev-check` warned that it predates local `main` |
| Node | `v25.4.0` |
| WG Pi plugin compat | `0.1.0`; embedded cache build ready; console setting not wired |
| Pi auth | `openai-codex` OAuth record present (field names checked; no values printed) |

`pi --list-models` reported:

| Provider/model | Context | Max output | Thinking | Images |
|---|---:|---:|---|---|
| `openai-codex/gpt-5.6-luna` | 372K | 128K | yes | yes |
| `openai-codex/gpt-5.6-terra` | 372K | 128K | yes | yes |
| `openai-codex/gpt-5.6-sol` | 372K | 128K | yes | yes |

Commands (outputs above were metadata-only):

```bash
pi --version
wg --version
node --version
for m in gpt-5.6-luna gpt-5.6-terra gpt-5.6-sol; do
  pi --list-models "$m"
done
jq 'to_entries | map({provider:.key, fields:(.value|keys)})' \
  ~/.pi/agent/auth.json
wg pi-plugin status
wg dev-check
```

The successful credentialed calls below are the functional auth check for each model. No token, account ID, session ID, or raw credential is included here.

## Direct Pi JSON matrix

### Method

Two trials per model and prompt class were run with a 120-second process bound, three calls at a time. Prompts represented:

- assignment: select a programmer from two candidates and return WG-like assignment JSON;
- evaluator: score a small parser change using WG's evaluation dimensions;
- FLIP inference: reconstruct a parser task from artifact/test evidence;
- FLIP comparison: score actual vs inferred parser prompts using FLIP dimensions.

The invocation was:

```bash
printf '%s' "$PROMPT" | timeout 120 pi \
  --mode json --print --no-session --no-tools \
  --no-extensions --no-skills --no-prompt-templates --no-context-files \
  --thinking low --provider openai-codex --model "gpt-5.6-$MODEL"
```

Raw streams were retained only under `/tmp/pi-terra-flip-probe-direct/` for the duration of the probe. The committed report contains only aggregate/sanitized fields.

### Trial results

`status/content` is process exit status followed by whether non-empty assistant text was present. Latency and total tokens are shown as trial 1 / trial 2.

| Model | Prompt | status/content (T1,T2) | Latency ms (T1/T2) | Total tokens (T1/T2) |
|---|---|---|---:|---:|
| Luna | assignment | `0/yes, 0/yes` | 4498 / 4313 | 557 / 556 |
| Luna | evaluator | `0/yes, 0/yes` | 6219 / 7196 | 692 / 719 |
| Luna | FLIP inference | `0/yes, 0/yes` | 5088 / 4626 | 512 / 512 |
| Luna | FLIP comparison | `0/yes, 0/yes` | 5202 / 5311 | 584 / 583 |
| Terra | assignment | `0/yes, 0/yes` | 4223 / 4243 | 546 / 546 |
| Terra | evaluator | `0/yes, 0/yes` | 5727 / 6015 | 641 / 663 |
| Terra | FLIP inference | `0/yes, 0/yes` | 5130 / 4326 | 510 / 504 |
| Terra | FLIP comparison | `0/yes, 0/yes` | 4877 / 5447 | 583 / 586 |
| Sol | assignment | `0/yes, 0/yes` | 4972 / 4418 | 547 / 547 |
| Sol | evaluator | `0/yes, 0/yes` | 6905 / 5987 | 671 / 666 |
| Sol | FLIP inference | `0/yes, 0/yes` | 4798 / 6248 | 528 / 531 |
| Sol | FLIP comparison | `0/yes, 0/yes` | 4886 / 7448 | 584 / 630 |

Aggregate observations:

| Model | Calls with exit 0 + content | Median latency | Total tokens | Pi-reported cost |
|---|---:|---:|---:|---:|
| Luna | 8/8 | 5145 ms | 4715 | $0.008030 |
| Terra | 8/8 | 5004 ms | 4579 | $0.018035 |
| Sol | 8/8 | 5480 ms | 4704 | $0.039820 |

This is a reliability probe, not a statistically meaningful quality benchmark. Two trials per cell bound cost while testing repeated calls; they cannot rule out a low-frequency intermittent empty.

## Raw event diagnosis

Every direct stream had zero invalid JSON lines, and all 24 final text responses parsed as the requested role-specific JSON shape. Every stream had the same unique event sequence:

```text
session, agent_start, turn_start, message_start, message_end,
message_update, turn_end, agent_end, agent_settled
```

The session event used Pi stream version 3. The authoritative final event looked structurally like:

```json
{
  "type": "turn_end",
  "message": {
    "role": "assistant",
    "provider": "openai-codex",
    "model": "gpt-5.6-terra",
    "content": [{"type": "text", "text": "<present>"}],
    "usage": {
      "input": 468,
      "output": 42,
      "cacheRead": 0,
      "totalTokens": 510,
      "cost": {"total": 0.0018}
    }
  }
}
```

This matches WG's current parser. `src/stream_event.rs:464-635` reads final text and usage from `turn_end.message`; `src/service/llm.rs:1127-1145` translates the captured stdout, rejects an empty `final_text`, and returns translated usage. The current event shape therefore does **not** demonstrate a systematic Pi/WG parser mismatch.

### Historical empty-response classification

The production log for the failed Luna FLIP inference contained `Empty response from pi CLI`. That exact error can only occur after the Pi subprocess exited successfully and WG's translation yielded empty final text (`src/service/llm.rs:1112-1135`). It was followed by a cross-system fallback attempt that failed with 403.

What the old record does **not** contain is the raw stdout passed to `translate_pi_stream`; `call_pi_cli` captures it in memory and discards it when final text is empty. Therefore:

- authentication/process spawn failure is excluded for that specific attempt (it would have produced the earlier non-zero-exit diagnostic);
- a permanent lack of Luna capability is contradicted by 8/8 direct content-bearing calls and a successful real Luna FLIP run here;
- a current stable parser incompatibility is contradicted by all current event streams;
- intermittent upstream empty content, intermittent Pi emission, and an unrecorded event-shape variation remain distinguishable only with raw NDJSON capture on the failure path.

**Required discriminating follow-up:** on empty final text, persist a sanitized diagnostic containing exit code, event-type counts, `turn_end` count, assistant content block types, usage, and stderr (never content or auth). That will locate the next empty at model/API vs Pi emitter vs WG parser without retaining sensitive prompts.

### Non-hermetic extension discovery found during the WG probe

Running the isolated graph with the normal console HOME initially failed before a model call:

```text
Extension "/home/bot/wg/pi-plugin/dist/index.js" error:
Provider wg: "apiKey" or "oauth" is required when defining models.
```

`call_pi_cli` currently supplies `--no-tools --no-session` but not the discovery-disabling flags used in the direct probe (`src/service/llm.rs:1054-1103`). The model was not responsible. For the valid isolated run, the probe used a temporary HOME containing only a mode-0600 copy of the existing Pi auth file and no settings/extensions. This temporary credential copy was not printed, committed, or registered as an artifact.

This is another reason agency one-shots should be hermetic: explicit extensions, skills, prompt templates, and context files are irrelevant to no-tool evaluator calls and can fail before the selected model is reached.

## Actual isolated WG FLIP plumbing

### Setup

Each model received a separate graph under `/tmp/wg-flip-{luna,terra,sol}/.wg`. The production daemon and production graph were never stopped, reloaded, or reconfigured.

Representative setup (repeated with each model):

```bash
D="/tmp/wg-flip-$MODEL/.wg"
wg --dir "$D" init --no-agency
wg --dir "$D" config --local \
  --auto-assign false --auto-evaluate false --flip-enabled true \
  --set-model flip_inference "pi:openai-codex:gpt-5.6-$MODEL" \
  --set-model flip_comparison "pi:openai-codex:gpt-5.6-$MODEL" \
  --no-reload
wg --dir "$D" add "Implement parse_bool" --id synth \
  --description "Implement parse_bool accepting true and false; test both." \
  --independent --no-place
wg --dir "$D" claim synth
wg --dir "$D" artifact synth "/tmp/wg-flip-$MODEL/artifacts/parser.rs"
wg --dir "$D" log synth \
  "Implemented parse_bool for true/false; both focused tests pass."
wg --dir "$D" done synth

# PROBE_HOME contains only a temporary chmod-0600 copy of Pi auth.json.
HOME="$PROBE_HOME" wg --dir "$D" evaluate run synth --flip
```

### Results

| Model | Exit | End-to-end latency | Phase 1 text | FLIP score | Combined usage | Pi cost |
|---|---:|---:|---:|---:|---:|---:|
| Luna | 0 | 17,531 ms | 346 chars | 0.96 | 1838 in / 512 out | $0.00491 |
| Terra | 0 | 15,452 ms | 225 chars | 0.98 | 1824 in / 348 out | $0.00978 |
| Sol | 0 | 18,624 ms | 410 chars | 0.98 | 1856 in / 476 out | $0.02356 |

All three exercised WG prompt rendering, role routing, `call_pi_cli`, NDJSON translation, inference JSON extraction/deserialization, comparison JSON extraction/deserialization, evaluation persistence, and combined token accounting. No empty response occurred.

## EPIPE: independent reproduction

Closing the stdout reader after Pi's first NDJSON event reproduces the reported Node failure with Luna on a trivial `OK` prompt:

```bash
set -o pipefail
printf '%s' 'Reply with exactly OK.' | HOME="$PROBE_HOME" pi \
  --mode json --print --no-session --no-tools --no-extensions \
  --no-skills --no-prompt-templates --no-context-files \
  --thinking low --provider openai-codex --model gpt-5.6-luna \
  2>/tmp/pi-epipe.stderr | head -n 1
printf '%s\n' "${PIPESTATUS[*]}"
```

Observed statuses were `0 1 0` (producer shell, Pi, `head`), and Pi stderr contained:

```text
Error: write EPIPE
.../dist/core/output-guard.js:15:40
Emitted 'error' event on Socket instance
Node.js v25.4.0
```

The first `session` line had already been delivered. `output-guard.js` retries only `ENOBUFS`, `EAGAIN`, and `EWOULDBLOCK`; the closed pipe raises `EPIPE`, and the Socket emits an unhandled error. The same mechanism explains why a worker can finish meaningful work or print a real compile failure and then exit 1 when its downstream reader closes. Model quality cannot prevent or repair a closed stdout consumer.

## `FailedPendingEval` respawn storm: independent diagnosis

The production evidence captured a complete, successful FLIP evaluation for `land-luca-pr-49`:

```text
FLIP Score: 0.82
Saved to: .../eval-land-luca-pr-49-....json
__WG_TOKENS__:{...}
Error: Cannot mark '.flip-land-luca-pr-49' as done: blocked by ...
  - land-luca-pr-49 ... FailedPendingEval
```

Subsequent agents recorded additional scores (0.83 and 0.81) before another attempt encountered the intermittent empty. The first successful result should have ended the child pipeline.

The source explains the asymmetry:

- readiness deliberately treats both `PendingEval` and `FailedPendingEval` as satisfied for system children (`src/query.rs:339-374`, also `:423-445`), so `.flip-*` is spawnable;
- manual completion exempts a system child's blocker only when the parent is `PendingEval`, not `FailedPendingEval` (`src/commands/done.rs:1434-1484`, especially `:1458-1461`), so that spawned child cannot become Done;
- reconciliation sees a nonterminal/failed child and dispatches it again, even though a durable evaluation already exists.

This is a deterministic lifecycle defect. Terra must not be credited with fixing it. The graph already tracks the implementation task `fix-failedpendingeval-and`, whose acceptance criteria include bounded retries, durable-result consumption, multi-tick no-growth validation, and a permanent smoke scenario.

## Failure classification

| Observation | Layer | Evidence | Model-quality implication |
|---|---|---|---|
| Current 24 direct calls | Model/API + Pi JSON | 24/24 exit 0 with content and usage | All three currently usable |
| Current three WG FLIP runs | WG one-shot + parser + FLIP | 3/3 complete and persist evaluations | Terra is viable; Luna is not permanently incapable |
| Historical Luna empty | Unresolved within successful Pi call | Pi exit succeeded; WG translated no final text; raw NDJSON absent | Do not infer general Luna weakness from this alone |
| Extension provider startup error | Pi discovery/config | failure occurs before selected model call | No model implication |
| `output-guard.js` EPIPE | Node/Pi stdout transport | independently reproduced by early reader close | No model implication |
| successful FLIP repeatedly respawned | WG lifecycle | durable score saved, then child `wg done` blocked by `FailedPendingEval` | No model implication |

## Recommended Pi role map and fallback

### Role map

| Role | Default | Reason |
|---|---|---|
| assignment, triage, placement, routine compaction | **Luna** (`low`) | 8/8 direct success; cheapest; adequate for bounded classification/selection |
| evaluator, reviewer, FLIP inference, FLIP comparison | **Terra** (`low` or `medium` when needed) | 8/8 direct plus real FLIP success; better capability/cost balance than Sol |
| task worker, hard verification, creator/evolver, escalation | **Sol** (`high`/`xhigh` by task) | strongest tier; reserve its materially higher cost for generative or high-stakes work |

**Answer:** yes, Terra should run FLIP/evaluation/review by default in the Pi OpenAI-Codex profile, but only after the `FailedPendingEval` completion bug is fixed and FLIP retries are bounded. Luna remains appropriate for assignment/triage and similarly short, structured decisions. It should not be the default for long semantic reconstruction/review prompts given the historical intermittent empty and the availability of Terra.

### Explicit same-system fallback

Use one monotonic capability chain inside Pi/OpenAI-Codex:

```text
Luna → Terra → Sol → loud retryable failure
```

Start at the role's default and move only to the right:

- Luna roles: Luna, then Terra, then Sol;
- Terra roles: Terra, then Sol;
- Sol roles: Sol, then fail/retry or require explicit operator action.

The chain must be explicitly configured, attempt-bounded, logged with role/model/failure layer, and never switch handlers or providers. A formatting failure may retry the same model before escalation; a startup/auth/transport failure should not consume three model attempts when no selected model was reached.

## Rollout and rollback

### Rollout gates

1. Land the `FailedPendingEval`/child-completion fix and permanent multi-tick smoke test.
2. Make Pi agency one-shots hermetic or otherwise prevent unrelated extension discovery.
3. Add sanitized empty-stream diagnostics (event counts/block types/usage, no prompt/content/auth).
4. Configure Terra for evaluator, reviewer, FLIP inference, and FLIP comparison; leave Luna on bounded weak roles and Sol on strong roles.
5. Enable FLIP first on an isolated/canary graph with a hard attempt cap and verify no agent-count growth after terminal evaluation.

### Rollback

If Terra shows elevated empty/invalid-JSON rates, disable FLIP scheduling and return the affected Pi roles to Sol while preserving the same Pi/OpenAI-Codex system. Do not change Luna's bounded assignment/triage role based on a Terra rollback. Preserve failed one-shots as retryable with diagnostics; do not silently fabricate or drop verdicts. Once transport/lifecycle fixes are verified, canary Terra again before broad re-enable.

## Production protection and sanitization

The probe issued no production `wg config`, profile activation, daemon reload, pause, or restart. At start and finish, the active `pi-codex` profile still had Sol workers and Luna as the fast tier; the already-existing temporary mitigation kept production evaluator/reviewer/FLIP roles on Sol. All probe graph writes used explicit `/tmp/.../.wg` directories. Temporary raw streams and the temporary auth copy were not committed or registered as artifacts. This report contains no raw auth material, account IDs, access/refresh tokens, session IDs, prompt transcripts from production, or assistant reasoning.
