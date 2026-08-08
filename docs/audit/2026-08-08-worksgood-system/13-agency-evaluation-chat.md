# Agency, evaluation, functions, chat, and adaptive intelligence

**Audit date:** 2026-08-08

**Audit snapshot:** `b0892ea7496fd2cc8f641417a3d8e33ca9add369`

**Evidence checkout:** `98b319c36aa8a21fd4506fc7469fe6d58978cdda`; its only diff from the snapshot is this audit's charter README

**Evidence checked through:** 2026-08-08T10:35:14Z

**Scope:** agency schemas, initialization, assignment, evaluation/FLIP, evolution, trace functions and memory, chat/concierge/session handling, context assembly, and human binding. Federation and generic model-handler details are covered elsewhere except where they bound authority here.

## 1. Executive abstract

**`[FACT]`** WorksGood contains two different evaluation/learning planes. The older agency plane stores compositional identities and standalone evaluation JSON under `.wg/agency/`; its prompts and score propagation are in `src/agency/` and `src/commands/{assign,evaluate}.rs`. The newer completion plane stores candidate-bound `EvaluationRecord`s on graph tasks and runs bounded/deep-readonly lanes without synthetic `.evaluate-*` workers (`src/evaluation/mod.rs:1-235`, `bounded.rs:371-615`, `deep.rs:291-698`). Chat/session continuity and trace-function memory add independent persistence planes.

**`[INFERENCE]` (high confidence)** Authority is strongest where it is explicit and candidate/attempt bound: bounded/deep evaluation, attended-chat authority, runtime ledgers, and confirmed human reply routing. It is weakest where old adaptation meets new lifecycle semantics. Automatic candidate-bound verdicts do not call the agency performance recorder, while auto-evolution counts only `.wg/agency/evaluations/*.json`. Thus the automatic evaluation path does not close the documented evaluation → performance → evolution loop (`AGENCY-004`).

**`[CONTRADICTION]`** “Content-hashed, immutable identity” is not a literal current contract. Role hashes exclude description; tradeoff hashes exclude acceptable/unacceptable tradeoffs. Edits to excluded behavioral fields retain an ID, while edits to included fields rehash and delete the old file, potentially stranding agent references (`AGENCY-002`).

**`[CONTRADICTION]`** Current source/tests say agency decisions are receipts and publication creates no `.assign-*` work. Another passing integration target privately copies a retired auto-assign subgraph instead of exercising production. The coordinator does not call the dormant lightweight assignment function. `auto_assign` is configurable, but no inspected automatic dispatch path uses it to select an agency identity (`AGENCY-003`).

**`[VERIFIED]`** Eight selected targets passed in a worker-control-neutral environment: agency schema (5), agency lifecycle (5), agency pipeline (34 passed/5 ignored), auto assignment (22), evaluation recording (30), deep-readonly FLIP (9), trace functions (64), and context scope (21). `integration_trace_function_layers` failed 3/53 because old `Done` fixtures lack now-required GraphSave completion evidence. `integration_chat` failed 3/9 daemon round-trip/concurrency tests with two-second IPC timeouts. These are execution evidence, not automatic product attribution.

**Priority decision:** make candidate-bound evaluation the sole authority, or explicitly retain two lanes; then define an exactly-once projection of accepted verdicts into agency learning. Otherwise gating can work while adaptive assignment/evolution stays starved.

## 2. Subsystem map

| Subsystem | Primary state | Model/prompt boundary | Gate/authority | Evidence |
|---|---|---|---|---|
| Agency primitives/composition | `.wg/agency/primitives/**`, `cache/{roles,agents}` | role + components + outcome + tradeoff become identity prompt | hashes, scope/work-pool eligibility | `src/agency/types.rs:16-630`; `hash.rs:15-67`; `prompt.rs:29-341` |
| Init/evolution | starter YAML, config hashes, `evolver_state.json`, deferred JSON | evolution prompt uses evaluation files and strategy | safe strategy/budget; deferral | `agency_init.rs:14-235`; `evolver.rs:1-234`; `commands/evolve/deferred.rs:14-270` |
| Assignment | `Task.agent`, assignment YAML/provenance | manual `--auto` is deterministic history ranking; dormant LLM assigner is weak-tier | work-pool exclusion; explicit human pin wins with warning | `commands/assign.rs:171-585`; `commands/service/assignment.rs:118-565` |
| Legacy evaluation | `.wg/agency/evaluations/*.json`, inline performance arrays | one-shot evaluator/two-call FLIP | legacy score or LLM gate | `commands/evaluate.rs:474-1690`; `agency/eval.rs:49-211` |
| Candidate evaluation | task `evaluation_records`, finalization receipts, evidence CAS | exact route; bounded no-tools or deep observation-only Pi | source/route digest, attempt fence, exact-candidate consumption | `evaluation/mod.rs:30-394`; `bounded.rs:417-1014`; `deep.rs:60-698` |
| Trace functions/memory | `.wg/functions/*.yaml`, `.runs.jsonl`, provenance | static substitution or pre-existing planner output; memory text | input/plan validation, GraphSave check | `function.rs:29-348`; `func_apply.rs:69-465,612-733`; `function_memory.rs:98-214` |
| Context | assembled prompt | task > role > config > default `task`; clean < task < graph < full | quantity boundary, not trust classification | `context_scope.rs:1-70`; `spawn/context.rs:14-187` |
| Attended chat | graph `.chat-N`, UUID registry, inbox/outbox, vendor journal, runtime ledger | attended contract appended to composed prompt | explicit human request + actual tool/sandbox permissions | `coordinator_prompt.rs:1-77`; `text/attended_chat_contract.md:1-18`; `chat_runtime.rs:1-340` |
| Human binding | human agent, user board, Telegram binding, task wait/message | no model needed for confirmation/routing | confirmed sender and bot/agent/task match | `human_binding.rs:38-269`; `human_dispatch.rs:82-557` |
| Concierge | graph/profile/plugin/service state and recovery marker | selected strong/weak routes; attended Pi can select its chat route | authenticated executable; transaction/reconcile plan | `concierge.rs:34-156,244-307,920-1271,1649-1819` |

### 2.1 Identity/authority distinctions

**`[FACT]`** “Identity” means at least: (1) agency `Agent` hash over `(role_id, tradeoff_id)` (`hash.rs:56-67`); (2) chat UUID/aliases plus optional agent binding (`chat_sessions.rs:78-133,740-805`); (3) runtime evidence containing graph, task, UUID, tmux session, executor/route/reasoning/session dir (`chat_runtime.rs:41-117,296-340`); and (4) confirmed Telegram sender binding (`human_binding.rs:38-102`). **`[INFERENCE]`** Agency hashes are descriptive compositions, not signing identities/capabilities. Chat authority comes from the attended system contract and tool sandbox, while human reply authority comes from binding plus lifecycle fences.

### 2.2 Flow A — initialize, assign, assemble prompt

```text
agency init -> seed primitives/roles/tradeoffs + meta agents
            -> enable auto_evaluate; leave auto_assign false
manual wg assign TASK --auto
            -> exclude system evaluators -> rank scoped historical scores
            -> Task.agent + assignment YAML + provenance
spawn       -> Agent -> Role/Tradeoff -> Components/Outcome
            -> scope task > role > config > task default
            -> append bound-session summary if present
```

**`[FACT]`** Init enables `auto_evaluate`, leaves `auto_assign` false, and clears inactive routes if no execution system was selected (`agency_init.rs:14-235,617-633`). Manual `--auto` performs max-score ranking, not an LLM call (`assign.rs:205-393`). Spawn resolves assigned identity and session memory independently (`service/executor.rs:1221-1386`).

**`[UNCERTAINTY]`** A path outside the searched coordinator might use `auto_assign`, but repository-wide search found no production caller of `run_lightweight_assignment`, `determine_assignment_path`, or `design_experiment`. Falsify by an isolated daemon scenario that publishes an unassigned task, invokes no `wg assign`, and proves `Task.agent` plus a persisted receipt.

### 2.3 Flow B — completion and evaluation

```text
attempt creates immutable candidate
 -> LazyEvaluationSelection resolves bounded/deep policy
 -> mint EvaluationRecord bound to candidate, attempt, dependencies,
    validation, exact route
 -> lane claims record by graph CAS
bounded: evidence manifest -> no-authority adapter -> strict verdict
         -> same transaction consumes and accepts/rejects source
deep: exact candidate -> four observation-only tools + budgets
      -> evidence-linked report -> consume once -> exact candidate promotion
```

**`[FACT]`** Selection excludes system/shell/paused/draft-like work and promotes coding hard-gate authority to deep (`evaluation/mod.rs:244-394`). Bounded records route, attempt, usage, manifest, verdict, and consumed ID and rechecks source/route/state (`bounded.rs:417-1002`). Deep explicitly denies writes, arbitrary command, network, credentials, authoring identity, source-session reuse, and live worktree access (`deep.rs:60-160`). **`[VERIFIED]`** `integration_deep_readonly_flip` passed 9/9 (`tests/integration_deep_readonly_flip.rs:338-949`).

The separate legacy `wg evaluate` builds a self-contained prompt, calls a lightweight route, optionally validates commands, writes an agency `Evaluation`, propagates performance, then applies legacy gates (`commands/evaluate.rs:474-1187`; `agency/eval.rs:49-211`). Legacy FLIP reconstructs a prompt and compares original intent (`evaluate.rs:1190-1690`).

### 2.4 Flow C — extract/apply/adapt trace function

```text
completed traces -> func extract -> function YAML + provenance
func apply -> resolve/validate inputs -> render prior memory
           -> consume already-verified planner task if present,
              otherwise static fallback
           -> atomic graph add + provenance + tracking row
make-adaptive -> reconstruct RunSummary from graph/provenance/evaluations
              -> append summaries + add memory config/version 3
```

**`[FACT]`** The model carries extraction provenance, static tasks, planning/constraints, memory, and export visibility (`function.rs:29-348`). Apply uses memory before graph creation (`func_apply.rs:69-203`). Generated topology is consumed only from an already-existing planner task with `Done` plus verified GraphSave; otherwise static templates return (`:612-685`). **`[VERIFIED]`** static trace functions passed 64/64. Layer tests passed 50 and failed 3 because fixtures still treated raw `Done` as successful memory; source now quarantines it as `NeedsReconciliation` (`function_memory.rs:410-425`).

### 2.5 Flow D — attended chat/session memory

```text
chat create -> validate execution -> .chat-N full context/full exec
            -> UUID + aliases -> session/runtime ownership
prompt = neutral project prompt (minus retired denylist) + attended contract
human send -> flocked inbox monotonic id -> vendor journal/outbox/runtime events
restart -> persistent tmux/session reattach
optional agent binding -> SessionMeta.agent_id -> next spawn reads
                          session-summary.md as bound memory
```

**`[FACT]`** The attended contract permits explicit human-directed reads/writes/tests/delegation while preserving actual restrictions (`text/attended_chat_contract.md:1-18`). Composition removes known retired denylist bodies (`coordinator_prompt.rs:1-77`). Create validates route/binary before mutation and treats attended bare Pi as the only route-free LLM chat (`chat_cmd.rs:274-396`). Runtime evidence is explicitly not spawn authority (`chat_runtime.rs:1-14`). Agent → session is 1:1: binding clears that agent from other sessions (`chat_sessions.rs:740-764`).

### 2.6 Flow E — human onboarding/reply

```text
agency human add -> telegram Agent -> user board -> unconfirmed binding
                 -> invitation -> YES/manual confirm
ready human task -> reserve attempt -> park on HumanInput -> notify
reply -> confirmed sender/bot/agent match -> freshest waiting task only
      -> message + wait receipt -> finalize + declared reply artifacts
```

**`[FACT]`** Numeric bindings match stable sender ID; handle bindings match normalized username. Confirmation accepts only bare `yes`/`y` and is idempotent (`human_binding.rs:65-126,224-269`). Reply routing requires confirmed binding and same dedicated bot/agent (`human_dispatch.rs:424-557`).

## 3. Findings

### `AGENCY-001` — compositional prompt identity is concrete, but access metadata is descriptive

- **Label/state:** `[FACT]`, shipped/current
- **Severity/confidence:** S4 informational; high
- Components, outcomes, tradeoffs, roles, and agents are persisted types; spawn renders role, desired outcome, acceptable tradeoffs, and constraints (`types.rs:328-554`; `prompt.rs:263-341`; `executor.rs:1221-1338`).
- `AccessControl { owner, policy }` is serialized (`types.rs:49-70,328-470`), but no enforcement was found in prompt/store paths. Treat it as metadata unless the federation audit identifies enforcement.
- File/URL component failures warn and continue (`prompt.rs:29-229`), so rendered identity may be incomplete rather than fail-closed.

### `AGENCY-002` — documented identity hashing/immutability conflicts with code

- **Label/state:** `[CONTRADICTION]`, current
- **Severity/likelihood/confidence:** S2; likely; high
- **Documentation claim:** role identity hashes description + skills + outcome; motivation hashes description + acceptable/unacceptable tradeoffs; changed content makes a successor while the original remains (`docs/manual/03-agency.md:1-133`).
- **Source fact:** role hash is sorted component IDs + outcome ID, excluding description; tradeoff hash is description only, excluding acceptable/unacceptable fields (`agency/hash.rs:32-67`). `role edit`/`tradeoff edit` modify authoritative YAML. Included-field changes save under a new hash and delete the old file; excluded-field changes save in place (`commands/role.rs:224-273`; `tradeoff.rs:229-276`).
- **Impact:** behaviorally different descriptions/constraints can retain one identity; rehash can delete a constituent still referenced by agents. No lineage is created.
- **Counterevidence:** constructors/tests confirm determinism for fields code actually hashes (`role.rs:295-309`; `integration_agency` passed 5/5), not the manual's broader contract.
- **Recommendation:** `AGENCY-REC-001`.

### `AGENCY-003` — `auto_assign` is surfaced but production reachability is absent

- **Label/state:** `[FACT]` + `[CONTRADICTION]`; partial/unknown reachability
- **Severity/likelihood/confidence:** S2; likely; high for call graph
- Config defaults false and init leaves it false (`config.rs:4106-4250,4362-4389`; `agency_init.rs:170-235,617-633`). The coordinator has no selection block; search finds no production call to `run_lightweight_assignment`. Manual `wg assign --auto` is deterministic max-score ranking (`assign.rs:205-393`).
- `integration_agency_pipeline.rs:947-1047` says publication creates no `.assign-*` tasks/edges. `integration_auto_assignment.rs:160-354` privately reimplements a retired `assign-*` builder and tests only that simulator; its comment cites obsolete `service.rs` lines. Both pass while specifying incompatible production stories.
- Manual says `auto_assign` acts after release (`docs/manual/03-agency.md:241-250`); quickstart says `.assign-*` LLM call (`quickstart.rs:235`).
- **Impact:** operators may enable a no-effect flag; dormant assigner/UCB learning has no live input.
- **Falsifying check:** isolated daemon E2E from §2.2.
- **Recommendation:** `AGENCY-REC-002`.

### `AGENCY-004` — automatic candidate verdicts do not feed agency learning/evolution

- **Label/state:** `[FACT]` + `[INFERENCE]`; partial
- **Severity/likelihood/confidence:** S1; likely; high
- Bounded/deep lanes persist task `EvaluationRecord`s/finalization receipts but never call `agency::record_evaluation` (`evaluation/mod.rs:194-235`; `bounded.rs:417-1014`; `deep.rs:391-698,1860-1965`).
- Agency performance propagation and inference exist only in `record_evaluation[_with_inference]`, called by legacy/manual evaluation/recording, assignment placeholder, and evolution meta-evaluation (`agency/eval.rs:49-211`; call-site evidence §7.3).
- Auto-evolution counts/loads `.wg/agency/evaluations/*.json`, not task records/receipts (`agency/evolver.rs:110-224`).
- **Inference:** modern automatic evaluation can gate a task but does not update assigned agent/role/tradeoff/components/outcome or increment evolution threshold. The documented adaptive loop is disconnected.
- **Counterevidence:** manual `wg evaluate` closes the old loop and `evaluation_recording` passed 30/30. That verifies the legacy store, not modern projection.
- **Recommendation:** `AGENCY-REC-003`.

### `EVAL-001` — candidate-bound evaluation has strong provenance/replay controls

- **Label/state:** `[FACT]`, `[VERIFIED]`; current
- **Severity:** S4 positive control
- Policy, candidate, route, attempt, usage, evidence, response digest, and consumption are explicit. Bounded delivery refuses stale/duplicate source/route/state. Deep is observation-only, budgeted, materialized, and replays acceptance without another model call (`evaluation/mod.rs:30-235`; `bounded.rs:417-1014`; `deep.rs:60-160,321-698`).
- `integration_deep_readonly_flip` passed 9/9. Human-flow smoke specs were inspected, not run: `dedicated_pi_bounded_evaluation_lane.sh:1-205`; `deep_readonly_flip_human_flow.sh:1-181`.

### `EVAL-002` — legacy evaluation validates range after mutating non-transactional learning state

- **Label/state:** `[FACT]`; compatibility path
- **Severity/likelihood/confidence:** S2; possible; high
- Legacy parses floats, applies validation overrides, constructs/records the evaluation, then `check_eval_gate` validates finite `[0,1]` score (`commands/evaluate.rs:842-1079,2253-2271`). Recorder writes evaluation JSON then independently updates agent, role, tradeoff, each component, and outcome (`agency/eval.rs:49-160`). No transaction or idempotency key spans files.
- **Impact:** malformed score can persist/propagate before command error; crash/retry can partially update or duplicate refs.
- `evaluation_recording.rs:755-797` preserves extreme/custom dimensions; no inspected crash/replay test exists.
- Individual YAML uses temp+fsync+rename (`agency/store.rs:35-68`), preventing torn files but not partial multi-entity state.

### `EVAL-003` — legacy FLIP spotlights untrusted diffs less safely

- **Label/state:** `[FACT]`; partial
- **Severity/likelihood/confidence:** S2; possible; medium
- Regular evaluator artifacts are 30-KB capped and enclosed in collision-resistant “data, never instructions” boundaries (`agency/prompt.rs:401-595`). Legacy FLIP uses the capped diff but inserts it in an ordinary markdown ` ```diff ` fence without the boundary/warning (`prompt.rs:784-911`); a diff can close the fence.
- Deep FLIP uses an explicit spotlight contract and closed tools (`evaluation/deep.rs:247-286,981-1228`). No adversarial legacy-model test was run.

### `FUNC-001` — generative schema exists, planner execution is externally staged

- **Label/state:** `[FACT]` + `[DOC-CLAIM]`; partial
- **Severity/likelihood/confidence:** S2; likely; high
- `func apply` never creates/runs `planner_template`. It consumes only a pre-existing task named `<prefix>-<planner_template_id>` that is `Done` with verified GraphSave; otherwise returns static templates (`func_apply.rs:612-685`).
- Design says planning runs first on instantiation and roadmap says “create planner task → parse → validate → create” (`docs/design/trace-function-protocol.md:76-109,500-521`); later implementation note omits the external prerequisite (`:559-572`).
- **Impact:** a generative definition silently behaves statically unless separately staged; even `static_fallback=false` does not fail merely because planner never ran.
- **Recommendation:** `FUNC-REC-001`.

### `FUNC-002` — apply tracking rows and adaptive summaries are incompatible JSON

- **Label/state:** `[FACT]` + `[INFERENCE]`; partial
- **Severity/likelihood/confidence:** S2; likely; high
- Apply appends `{applied_at, inputs, prefix, task_ids}` to `<id>.runs.jsonl` (`func_apply.rs:435-451,708-726`). V3 loader deserializes each line as `RunSummary`, requiring `task_outcomes`, `interventions`, `all_succeeded`, and silently drops failures (`function.rs:298-346`; `function_memory.rs:370-408`). Production completion does not call `build_run_summary`; make-adaptive/tests do.
- **Inference:** normal v3 apply creates a row future adaptive loads ignore. The operational ledger proves application, not usable outcome memory.
- Tests separately assert raw tracking rows and manually appended full summaries; none proves apply → completion → next apply learns (`func_apply.rs:1625-1699`).
- **Recommendation:** `FUNC-REC-002`.

### `CHAT-001` — attended and runtime authority are explicit

- **Label/state:** `[FACT]`; current
- **Severity:** S4 positive control
- Attended chat receives a human-directed full-tool contract; known retired denylist bodies are removed; runtime ledger is evidence, never spawn authority (`text/attended_chat_contract.md:1-18`; `coordinator_prompt.rs:1-77`; `chat_runtime.rs:1-14,41-117`; `chat_cmd.rs:768-975`).
- Real-TUI smoke specification was inspected, not run: `attended_chat_user_authority.sh:1-249`.

### `CHAT-002` — durable chat has multiple histories and unresolved concurrency signal

- **Label/state:** `[FACT]`, `[VERIFIED]`, `[UNCERTAINTY]`; current
- **Severity/likelihood/confidence:** S2; observed in audit environment; medium
- State spans inbox/outbox JSONL, plaintext `chat.log`, vendor journal, UUID registry, cursors, stream state, runtime ledger, and graph metadata (`chat.rs:1-60,246-317`; `commands/chat.rs:244-304`; `chat_sessions.rs:1-146`; `chat_runtime.rs:1-117`). History prefers vendor data then inbox/outbox (`commands/chat.rs:244-304`).
- Inbox/outbox are flocked and cursors atomic (`chat.rs:181-391`). Registry saves use unique temp files, but only coordinator registration locks the whole read-modify-write; the comment admits other concurrent writers can lose updates (`chat_sessions.rs:37-65,164-245,397-433`).
- `integration_chat` passed 6/9; round-trip, instant-wakeup, and concurrent-message tests timed out after 2s with EAGAIN. Real daemons plus concurrent audit load make attribution uncertain.
- Restart/resume smoke spec inspected, not run: `tui_stateful_chat_restart_resume.sh:1-207`.
- **Recommendation:** `CHAT-REC-001`.

### `CHAT-003` — bound summary enters worker prompt without provenance/safety gate

- **Label/state:** `[FACT]` + `[INFERENCE]`; current
- **Severity/likelihood/confidence:** S2; possible; high
- Binding reads `session-summary.md`; spawn inserts it verbatim as “Persistent Memory (your bound session)” with a prose disclaimer (`chat_sessions.rs:786-805`; `executor.rs:1351-1386`). No digest, author/source, injection scan, or spotlight delimiter is present.
- **Inference:** compromised/stale summary can influence later worker while framed as its own memory. Candidate evaluation and federated state have stronger content/review boundaries.
- **Recommendation:** `CHAT-REC-002`.

### `CONTEXT-001` — scope is deterministic but not a trust classification

- **Label/state:** `[FACT]`, `[VERIFIED]`; current
- **Severity:** S3
- Resolution is task > role > config > `task`; invalid stored strings silently fall through (`context_scope.rs:1-70`). Task includes dependencies/downstream/tags/messages; graph adds project/neighborhood; full adds full graph and `CLAUDE.md` (`spawn/context.rs:14-187`). Neighbor summaries are XML-fenced/capped; other sources have separate formatting.
- `integration_context_scope` passed 21/21. “More context” is not “more trusted context.”

### `HUMAN-001` — confirmed binding is a real authority check

- **Label/state:** `[FACT]`; current
- **Severity:** S4 positive control
- Onboarding stores unconfirmed binding before invitation; reply routing proves sender → confirmed binding → human agent → waiting task and matching dedicated bot (`agency_human.rs:150-268`; `human_dispatch.rs:424-557`). Parking uses lifecycle reservation rather than spawning AI (`:82-180`).
- Manual confirm is an operator assertion, not proof the person sent YES (`agency_human.rs:270-310`); it should remain explicitly audited.

### `HUMAN-002` — onboarding is race-safe but not transactional

- **Label/state:** `[FACT]` + `[CONTRADICTION]`; current
- **Severity/likelihood/confidence:** S2; possible; high
- Comment promises up-front validation prevents partial state, but command writes agent, initializes board, then writes binding (`agency_human.rs:126-214`). Later failure leaves earlier state; no rollback. Binding-before-DM does correctly close fast-YES race (`:195-235`, test `:438-476`).
- Retry may hit “human already exists” after a partial first attempt.
- **Recommendation:** `HUMAN-REC-001`.

## 4. Contradictions and drift

| ID | Claims in tension | Status |
|---|---|---|
| `AGENCY-DRIFT-001` | Manual hash/immutability (`manual/03-agency.md:80-133`) vs actual hash fields/destructive rename (`hash.rs:15-67`; `commands/{role,tradeoff}.rs`) | Open; source governs behavior, product semantics need decision. |
| `AGENCY-DRIFT-002` | Manual/quickstart auto-assignment vs no coordinator caller; stale simulator test vs current receipt test | Open; no synthetic auto-assignment is current tested publication behavior. |
| `AGENCY-DRIFT-003` | Manual says evaluation feeds evolution; automatic `EvaluationRecord` never enters agency evaluation store | Open/material; two stores work locally but do not compose. |
| `AGENCY-DRIFT-004` | `assign.rs:204` calls `--auto` “using LLM”; implementation performs historical max. Dormant service code uses an LLM. | Open terminology/reachability drift. |
| `EVAL-DRIFT-001` | Legacy manual evaluator/FLIP and candidate-bound bounded/deep evaluator have different schemas, containment, gates, and replay | Compatibility/migration status is not stated in one authority map. |
| `FUNC-DRIFT-001` | Design says planner runs on instantiate; source only consumes a pre-existing verified result | Open. |
| `FUNC-DRIFT-002` | Design says JSONL is shared apply/memory history; apply writes a row loader drops | Open defect/gap. |
| `FUNC-DRIFT-003` | Three layer tests expect raw `Done`; source requires GraphSave disposition | Stale fixtures; 3 observed failures. |
| `CHAT-DRIFT-001` | `chat_sessions.rs:1-28` says alias symlinks; `chat.rs:67-87` says aliases exist only in registry, while compatibility symlinks remain | Comment/version conflict; UUID remains canonical. |
| `CHAT-DRIFT-002` | `sessions-as-identity.md` is design; much shipped via a different tmux/runtime-ledger shape | Mark sections implemented/partial/superseded. |
| `HUMAN-DRIFT-001` | “never write partial state” vs sequential non-transactional writes | Open comment overclaim. |

## 5. Risks and gaps

| ID | Severity | Likelihood | Risk/gap | Uncertainty |
|---|---:|---|---|---|
| `AGENCY-RISK-001` | S1 | likely | Modern gating succeeds while automatic agency learning/evolution receives no score. | No hidden projection found; call search is strong, not runtime proof. |
| `AGENCY-RISK-002` | S2 | likely | Unhashed behavioral edits preserve ID; hashed edit deletes referenced constituent. | No destructive edit executed. |
| `AGENCY-RISK-003` | S2 | likely | `auto_assign` UX promises behavior absent from dispatch; copied simulator keeps old story green. | Needs daemon E2E. |
| `EVAL-RISK-001` | S2 | possible | Legacy crash/retry or malformed score partially/duplicatively corrupts performance. | No fault injection. |
| `EVAL-RISK-002` | S2 | possible | Legacy FLIP diff can close markdown spotlight fence. | No adversarial model run. |
| `FUNC-RISK-001` | S2 | likely | V3 applications do not become usable memories; planner config silently behaves statically. | Full adaptive production flow not run. |
| `CHAT-RISK-001` | S2 | observed/unknown | Chat IPC timed out under load; registry has non-coordinator RMW races; histories complicate replay. | Requires isolated repeat/logs. |
| `CHAT-RISK-002` | S2 | possible | Bound summary is trusted as own memory without review/provenance. | Local ownership may lower likelihood. |
| `HUMAN-RISK-001` | S2 | possible | Partial onboarding strands agent/board/binding and makes retry non-idempotent. | No failure injection. |
| `CONTEXT-GAP-001` | S3 | likely | Scope controls quantity, not provenance; invalid persisted scope silently falls back. | CLI rejects normal invalid input. |
| `TEST-GAP-001` | S2 | observed | Auto-assign target tests copied logic; layer target stale; chat target environment-sensitive. | Testing audit should assess suite-wide. |
| `DOC-GAP-001` | S2 | likely | No current map explains two evaluation planes and several identity/memory meanings. | Documentation audit owns repository-wide freshness. |

## 6. Recommendations

1. **`AGENCY-REC-001` — `[RECOMMENDATION]` (P0):** publish exact hash equations and classify every field as hashed, mutable metadata, or immutable. Preserve old constituents/successor lineage; never strand references. **Accept:** changing any prompt-visible identity field either changes ID with old entity retained or is explicitly documented/tested mutable metadata.
2. **`AGENCY-REC-002` — `[RECOMMENDATION]` (P0):** restore an attempt-bound automatic assignment receipt or deprecate/remove `auto_assign` and dormant LLM/UCB story. Replace simulator tests with daemon E2E. **Accept:** enabling flag has one documented effect proved by `Task.agent`, receipt, exact route, or fails loudly.
3. **`AGENCY-REC-003` — `[RECOMMENDATION]` (P0):** define exactly-once candidate-bound projection from consumed modern verdict into normalized agency learning. **Accept:** one verdict updates the assigned composition once; retry/superseded candidate is inert; evolver sees it.
4. **`EVAL-REC-001` — `[RECOMMENDATION]` (P1):** until legacy retirement, validate before persistence; make propagation idempotent/transactional by evaluation/candidate ID; spotlight legacy FLIP diff. **Accept:** invalid output cannot mutate state; crash/retry yields one evaluation.
5. **`FUNC-REC-001` — `[RECOMMENDATION]` (P1):** document external planner staging or implement create/resume protocol. **Accept:** `static_fallback=false` cannot silently create static topology when no planner ran.
6. **`FUNC-REC-002` — `[RECOMMENDATION]` (P0):** unify tracking and adaptive summaries under versioned schema or separate files; add apply → verified completion → summarize once → next apply test; update stale GraphSave fixtures. **Accept:** no silent parse loss; layer target passes.
7. **`CHAT-REC-001` — `[RECOMMENDATION]` (P1):** lock every registry read-modify-write; rerun failed chat tests with isolated socket/tmux state and logs. **Accept:** concurrent create/alias/bind cannot lose entries and target passes repeatedly.
8. **`CHAT-REC-002` — `[RECOMMENDATION]` (P1):** make bound memory content-addressed with session/author/model/time provenance and prompt-data boundary; use review policy for non-same-session lineage. **Accept:** mutation detected and origin inspectable.
9. **`HUMAN-REC-001` — `[RECOMMENDATION]` (P1):** make onboarding restartable via transaction/reconciliation covering agent, board, binding, invitation. **Accept:** failure after each step converges on retry; manual confirm attributed.
10. **`DOC-REC-001` — `[RECOMMENDATION]` (P0):** publish one evaluation authority map and qualify “identity” as agency composition, chat session, runtime, channel binding, or cryptographic identity.
11. **`AGENCY-DEC-001` — `[RECOMMENDATION]` (P0 human decision):** decide whether performance is immutable identity content, mutable local evidence, or a context-partitioned external ledger before `AGENCY-REC-003`.
12. **`CHAT-DEC-001` — `[RECOMMENDATION]` (P1 human decision):** name canonical replay/export evidence: vendor journal, inbox/outbox, runtime ledger, or declared composition.
13. **`HUMAN-DEC-001` — `[RECOMMENDATION]` (P1 human decision):** state assurance of handle vs stable numeric Telegram ID and whether manual confirmation is allowed for high-impact tasks.

## 7. Evidence appendix

### 7.1 Snapshot/environment and method

**`[VERIFIED]`** Production source at checkout equals the pinned snapshot: `git diff --name-only b0892ea7..98b319c3` returned only the subsequently added audit charter. Toolchain: `rustc 1.96.0`, `cargo 1.96.0`. Static method: read schemas/stores/prompts, traced CLI → coordinator/executor → persistence, searched callers/fields, cross-checked manuals/designs/tests/smoke scripts, then executed representative tests. No network/model call, product mutation, daemon action, or destructive identity edit was used.

Initial inherited `WG_TASK_ID`, `WG_AGENT_ID`, `WG_AGENT_DIR`, and related worker-control variables caused false failures by activating worker guards. All reported test results below use:

```sh
env -u WG_TASK_ID -u WG_AGENT_ID -u WG_AGENT_DIR -u WG_TASK_DIR \
    -u WG_ROOT_TASK_ID -u WG_PARENT_TASK_ID -u WG_EXECUTOR_TYPE \
    -u WG_MODEL -u WG_ENDPOINT -u WG_DAEMON_MANAGED \
    cargo test --test <target> -- --test-threads=1
```

### 7.2 Executed tests

| Target | Result | What it establishes / does not establish |
|---|---:|---|
| `agency_schema_fields` | 5 passed | serialization fields/defaults; not runtime use |
| `integration_agency` | 5 passed | CRUD/lifecycle basics |
| `integration_agency_pipeline` | 34 passed, 5 ignored | current receipt/no-synthetic-publication contract and route config; ignored auto-evolution paths not evidence |
| `integration_auto_assignment` | 22 passed | helper behavior; many tests use copied retired builder, so not production reachability |
| `evaluation_recording` | 30 passed | legacy recorder and metric semantics; not modern projection |
| `integration_deep_readonly_flip` | 9 passed | deep policy, exact candidate, budgets, restart, no-authority tools |
| `integration_trace_functions` | 64 passed | extraction/static apply/import/export basics |
| `integration_trace_function_layers` | 50 passed, 3 failed | failures at tests `711-733`, `964-1014`, `1280-1328`; current source requires GraphSave, fixtures only set `Done` |
| `integration_chat` | 6 passed, 3 failed | failures `chat_round_trip_storage`, `chat_instant_wakeup`, `chat_concurrent_messages`; 2s IPC EAGAIN timeouts |
| `integration_context_scope` | 21 passed | ordering, inclusion, override/fallback behavior |

**`[UNCERTAINTY]`** Chat failures were not isolated enough to distinguish product IPC defect, test resource contention, or audit-host load. Reproduce with a clean unique daemon/socket/tmux environment and capture service logs before classification.

### 7.3 Search/call-graph evidence

Representative repository-wide searches:

```text
rg "run_lightweight_assignment|determine_assignment_path|design_experiment" src
  -> only definitions/tests in src/commands/service/assignment.rs

rg "record_evaluation_with_inference|record_evaluation\\(" src --glob '*.rs'
  -> agency/eval.rs definitions/tests
  -> commands/assign.rs (placeholder), commands/evaluate.rs (legacy/manual),
     commands/evolve/mod.rs (meta-evaluation)
  -> no src/evaluation/{bounded,deep,mod}.rs caller

rg "auto_assign" src/service/coordinator.rs src/service src/commands/service
  -> no production coordinator assignment decision
```

**`[INFERENCE]`** Call-site absence is strong static reachability evidence, not proof against dynamically invoked external commands. The proposed daemon E2Es are the falsification mechanism.

### 7.4 Direct source/doc/test evidence index

| Topic | Evidence |
|---|---|
| schema + performance arrays | `src/agency/types.rs:16-124,328-630` |
| hashes | `src/agency/hash.rs:15-67` |
| store atomicity scope | `src/agency/store.rs:35-68,208-356` |
| prompt resolution/spotlighting | `src/agency/prompt.rs:29-341,401-595,784-911` |
| role/tradeoff edits | `src/commands/role.rs:149-273`; `tradeoff.rs:151-276` |
| init routes/defaults | `src/commands/agency_init.rs:14-235,617-633` |
| deterministic manual auto assign | `src/commands/assign.rs:205-393` |
| dormant LLM assigner | `src/commands/service/assignment.rs:118-565`; weak route `service/llm.rs:252-269` |
| modern evaluation record/policy | `src/evaluation/mod.rs:30-394` |
| bounded delivery | `src/evaluation/bounded.rs:417-1014` |
| deep capabilities/runner/consume | `src/evaluation/deep.rs:60-160,291-698,981-1228` |
| legacy record/gate order | `src/commands/evaluate.rs:842-1187,2253-2271`; `src/agency/eval.rs:49-211` |
| evolver input | `src/agency/evolver.rs:110-224` |
| function schemas/apply | `src/function.rs:29-348`; `src/commands/func_apply.rs:69-465,612-733` |
| memory schema/loader | `src/function_memory.rs:98-214,370-486` |
| attended prompt/runtime | `src/service/coordinator_prompt.rs:1-77`; `src/text/attended_chat_contract.md:1-18`; `src/chat_runtime.rs:1-340` |
| chat storage/registry | `src/chat.rs:1-391`; `src/chat_sessions.rs:1-245,397-433,740-805` |
| bound memory injection | `src/service/executor.rs:1351-1386` |
| context scope/assembly | `src/context_scope.rs:1-70`; `src/commands/spawn/context.rs:14-187` |
| human binding/replies | `src/agency/human_binding.rs:38-269`; `src/commands/service/human_dispatch.rs:82-557` |
| onboarding ordering | `src/commands/agency_human.rs:126-268,438-476` |
| manual claims | `docs/manual/03-agency.md:1-133,241-250` |
| trace design claims | `docs/design/trace-function-protocol.md:76-109,500-572` |
| session design | `docs/design/sessions-as-identity.md`; `docs/design/chat-agent-persistence.md` |
| authority migration | `docs/migration-attended-chat-authority.md`; `tests/smoke/scenarios/attended_chat_user_authority.sh` |

### 7.5 Claims not verified dynamically

- **`[UNCERTAINTY]`** No live evaluator/assigner model was called; prompt-injection susceptibility and route credential fallback were inspected, not exercised.
- **`[UNCERTAINTY]`** Concierge setup/recovery was statically traced but no operator-level profile/service transaction was run.
- **`[UNCERTAINTY]`** Human Telegram invitation, fast YES, and reply completion were not run against Telegram.
- **`[UNCERTAINTY]`** Smoke scripts were treated as executable specifications but not executed; selected integration tests are the only runtime evidence claimed.
- **`[UNCERTAINTY]`** This audit does not assert cryptographic identity, federation access-control, or review-gate correctness outside the crossings named above.

## 8. Conclusion

**`[INFERENCE]` (high confidence)** WorksGood has real, independently credible primitives for agency composition, candidate-bound evaluation, attended authority, human routing, context selection, and workflow reuse. The principal system risk is not absence of mechanisms but broken composition among generations of mechanisms: current verdicts do not become agency learning; auto-assignment documentation/tests do not prove a live path; adaptive apply rows are not adaptive summaries; and session memory bypasses newer provenance gates. Resolve those joins before adding more evaluator, planner, or memory sophistication.
