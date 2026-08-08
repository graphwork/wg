# Agency, federation, safety, execution, and trust synthesis

**Audit date:** 2026-08-08

**Audit snapshot:** `b0892ea7496fd2cc8f641417a3d8e33ca9add369` (production source and pre-existing documentation)

**Evidence checked through:** 2026-08-08

**Freshness:** snapshot-current. The three dependency artifacts were produced from snapshot-equivalent production trees; this synthesis independently spot-checked the material cross-boundary claims against the same primary source. See section 7.

**Scope:** WorksGood agency/persona identity, model authority, cryptographic identity, local trust assertions, capabilities, context exposure, content review, candidate evaluation, remote execution, learning/evolution, and human control as one authority system.

**Change boundary:** audit artifact only. No production source, tests, schemas, or pre-existing documentation were modified.

## 1. Executive abstract

**`[FACT]`** WorksGood does not implement one identity or one trust number that authorizes everything. It implements a **vector authority model** whose dimensions answer different questions:

1. an agency `Agent` hash selects a role/trade-off prompt composition;
2. a handler-first model route selects the process/silicon asked to reason;
3. a `wgid:` plus sigchain authenticates a cryptographic principal and its current keys;
4. local peer/provider registries assert trust for authorship and compute separately;
5. a UCAN-like capability grants an audience a named, expiring action over a resource;
6. context scope and sealing limit what information is disclosed;
7. review decides whether exact inbound bytes may be consumed;
8. candidate evaluation decides whether an exact task candidate may progress;
9. provider attribution, review/re-run, and a lease epoch decide whether a remote result may commit; and
10. an attended request or confirmed channel binding supplies human authority at specific local edges.

Primary enforcement sites include `src/agency/hash.rs:15-67`, `src/dispatch/handler_for_model.rs:1-137`, `src/identity/sigchain.rs:680-886`, `src/trust.rs:85-124`, `src/identity/custody.rs:294-486,681-805`, `src/review/mod.rs:326-461`, `src/evaluation/mod.rs:30-235`, and `src/commands/exec_fed_cmd.rs:851-979`.

**`[INFERENCE]` (high confidence)** The coherent system rule is therefore not “trusted agent may act.” It is:

```text
permit(operation, bytes, candidate) only if
  principal authentication
  AND local role-specific trust policy
  AND capability/scope/expiry
  AND information-release policy
  AND content-consumption policy
  AND candidate/result-integrity policy
  AND current lifecycle/lease fence
  AND any required human authorization
all pass at the consuming or canonical-write edge.
```

No one coordinate substitutes for another. A valid signature proves who signed, not safe content. `Verified` author trust reduces review depth, not graph-write authority. A strong model may improve judgment, but owns no key or capability by virtue of being strong. A task-scoped UCAN authorizes a write attempt, not correctness. A passing evaluator verdict does not make the author cryptographically trusted. This separation is the strongest conceptual property in the inspected implementation.

**`[FACT]`** This synthesis did not rerun product tests; it performed source inspection and static call-site searches only.

**`[VERIFIED]` (dependency evidence)** The leaves record bounded execution of selected agency/evaluation/context suites; 100 `identity::` tests and four isolated operator-mode federation smokes; and focused review (53), provider (54), trust (7), pilot (8), and remote-planner (1) tests. The exact commands, environments, failures, and limitations remain in `13-agency-evaluation-chat.md:329-416`, `14-federation-identity-security.md:325-473`, and `15-review-exec-pilot.md:272-353`.

**`[INFERENCE]` (high confidence)** The highest cross-system risk is not a missing cryptographic primitive. It is **authority collapse at the host boundary**: ordinary agents and attended chat can receive shell/write authority while the federation custodian reads root/recovery material from the same user's keystore in-process. The public bundle excludes private keys, but the local worker/custodian separation claimed by the security model is not enforced by a distinct UID, authenticated signer service, or HSM (`XAUTH-004`, S1).

**`[INFERENCE]` (high confidence)** The next structural risks are joins between otherwise credible planes:

- agency persona, agency `trust_level`, federated `agent_fields`, and `wgid:` have no enforced one-to-one mapping (`XAUTH-005`);
- accepted candidate evaluations do not feed agency learning/evolution (`XAUTH-006`);
- bound session summaries and other local memories can enter prompts without the provenance/review controls applied to federated content (`XAUTH-007`);
- a remote grant names a model, but an explicit command can replace it and the signed result omits actual backend/model provenance (`XAUTH-008`);
- review demotion, provider trust lowering, cryptographic revocation, and capability revocation are separate ledgers with no atomic incident response (`XAUTH-009`);
- remote accept consumes the epoch before graph accounting/finalization, and the normal coordinator still does not drive the provider lifecycle (`XAUTH-010`).

**`[RECOMMENDATION]` Priority decision:** treat authority as an explicitly typed vector and harden the joins before adding more evaluator, federation, or pilot sophistication. P0 work is: isolate custody; define persona↔principal↔human bindings; make security-relevant audit/accept state transactional; and either implement coordinator-driven remote execution or label it manual/experimental. Preserve the current fail-closed controls and the explicit spark/deferred boundaries while doing so.

## 2. Scope and unified authority/trust map

### 2.1 Inputs and synthesis disposition

**`[INFERENCE]` Local abstract (high confidence).** The three leaves agree that strong local mechanisms coexist with broken or manual composition. This synthesis adopts their enforcement findings, narrows broad “one trust dial,” “custody,” “quorum,” “complete federation,” and “turnkey pilot” wording, and leaves governance/product choices unresolved rather than selecting an authority by prose age.

| Dependency | Disposition in this synthesis | Stable findings retained |
|---|---|---|
| [`13-agency-evaluation-chat.md`](13-agency-evaluation-chat.md) | **Adopt:** persona hashes are descriptive, candidate evaluation is strongly bound, attended/human edges are explicit, and modern verdicts do not feed learning. **Narrow:** context scope is exposure quantity, never trust; “identity” must be qualified. | `AGENCY-001..004`, `EVAL-001..003`, `FUNC-001..002`, `CHAT-001..003`, `CONTEXT-001`, `CONCIERGE-001`, `HUMAN-001..002` |
| [`14-federation-identity-security.md`](14-federation-identity-security.md) | **Adopt:** self-certifying identity, root lock, envelope crypto, and attenuation are real controls; custody, recovery, inbox, handshake, freshness, state load, and governance are partial. **Reject as unqualified:** “worker cannot reach root,” “to is the ACL,” and “WG-Fed complete.” | `FED-001..014`, especially `FED-003/004/006/007/010/011/012/013` |
| [`15-review-exec-pilot.md`](15-review-exec-pilot.md) | **Adopt:** split trust, four class hooks, default-on gates at named entry points, strong CLI accept ordering, and manual coordinator/pilot seams. **Narrow:** “one dial” means one vocabulary/order with split assertions; deterministic Pass 2 is not an independent quorum; pilot real-host `up` is bootstrap. | `RXP-001..011` |

**`[FACT]`** All three artifacts cite direct source and clearly distinguish inspected from executed evidence. Their important contradictions are preserved in section 4 rather than averaged away.

### 2.2 The authority vector

**`[INFERENCE]` (high confidence)** A useful normalized record for any sensitive operation is:

```text
AuthorityContext {
  human_authorization,       // attended request or confirmed channel binding
  principal, key_position,  // wgid + verified sigchain/key
  persona,                   // agency Agent/Role/Tradeoff prompt composition
  execution_route,          // exact handler/provider/model/reasoning
  trust_assertions,          // author trust; provider trust; local demotions
  capability,               // can@with, audience, expiry, proof, revocation view
  information_release,      // context scope, sensitivity, sealing recipients
  content_verdict,          // exact content CID, source, depth, policy
  candidate_verdict,        // exact task candidate/attempt/route/evidence
  commit_fence,             // graph lifecycle generation or provider lease epoch
}
```

**`[FACT]`** The repository has representations for nearly every field, but not one durable structure binding them all. `EvaluationRecord` carries exact candidate and route provenance (`src/evaluation/mod.rs:82-217`); `RunGrant` carries principal, provider, model, two capabilities, sealed bundle, and lease (`src/providers/mod.rs:484-515`); `VerdictRecord` carries content CID, provenance, depth and trace but no signature or reviewer route (`src/review/verdict.rs:53-80`); agency `Agent` carries role/trade-off, model/provider preferences, a `TrustLevel`, and performance (`src/agency/types.rs:500-540`). The absence of one universal token is good; the absence of explicit cross-references is the seam.

### 2.3 Identity and actor distinctions

| Name in product | What it is | What it authorizes by itself | What it does not authorize |
|---|---|---|---|
| Agency `Agent.id` | SHA-256 of `(role_id, tradeoff_id)` | prompt/persona selection when assigned to a task | signing, graph write, provider placement, human identity |
| Role/component/outcome/trade-off IDs | partial content/composition hashes | lookup and prompt composition | immutable semantics; several prompt-visible fields are unhashed |
| Chat UUID/session/agent binding | local continuity and runtime ownership | session reattachment and bound-memory lookup | federation authorship or task spawn authority |
| Telegram binding | local sender→human-agent mapping after confirmation | routing a reply to the matching waiting human task | cryptographic `wgid:` continuity; handle binding has lower assurance than numeric ID |
| `wgid:` | genesis Ed25519 public key plus verified sigchain | authenticate principal/key lineage and signed envelopes | honesty, content safety, model strength, provider trust |
| Provider `wgid:` | same cryptographic identity enrolled in local provider registry | nothing from enrollment alone; eligibility after local policy | author trust; provider self-ad cannot raise local trust |
| Capability audience | `wgid:` receiving `can@with` until expiry | exactly the verified scope at a relying gate | correctness, safe content, current lease epoch |
| Model route | handler/provider/model/reasoning selection | which process/model is invoked | principal identity or permission |

**`[FACT]`** Agency IDs are not cryptographic principals. The hash equations omit role description, trade-off acceptable/unacceptable lists, component content/category, outcome criteria, and all model/trust fields (`src/agency/hash.rs:15-67`). `IdentityRecord.agent_fields` is optional signed metadata containing string fields for role, trust, executor, and capabilities (`src/identity/envelope.rs:44-51,61-80`), but mint currently fills `role_id` with the local identity name, `trust_level` with `"untrusted"`, and no capabilities (`src/commands/identity_cmd.rs:344-355`). Repository search found no production consumer that binds those fields to an agency `Agent`.

**`[FACT]`** `Agent.trust_level` reuses `graph::TrustLevel` (`src/agency/types.rs:1-6,500-540`), but the inspected automatic assignment ranks scoped performance after work-pool filtering and does not read that field (`src/commands/assign.rs:205-393`). By contrast, review and provider placement read local federation/provider registries (`src/trust.rs:85-124`; `src/providers/placement.rs:142-254`). Same enum spelling does not mean same authority source.

### 2.4 Model authority

**`[FACT]`** Handler-first route parsing decides the execution mechanism. `handler_for_model` derives Claude/Codex/Pi/external CLI/native routing from the model spec and keeps deprecated provider-leading forms lenient (`src/dispatch/handler_for_model.rs:1-137`). Agency one-shots resolve a role-specific route; built-in tiers and project worker routes do not themselves authorize evaluator/reviewer/FLIP/assignment execution (`src/service/llm.rs:252-285`). Modern candidate evaluation snapshots the exact handler/provider/route/reasoning and a digest (`src/evaluation/mod.rs:120-217`).

**`[INFERENCE]` (high confidence)** Model authority is **decision authority only at the gate that consumes its output**. A reviewer model may influence a content verdict; an evaluator model may influence candidate acceptance; a worker model may author a candidate. None gains signing/capability authority from model identity. Conversely, changing a model route can materially change behavior without changing agency `Agent.id`, `wgid:`, or capability audience. Security and evaluation records therefore need explicit route provenance wherever model quality matters.

### 2.5 Trust, capability, review, and evaluation are separate

**`[FACT]`** `TrustLevel` is an ordered local assertion (`Verified`, `Provisional`, `Unknown`; `src/graph.rs:2530-2541`). Author trust starts with the peer registry, defaults to `Unknown`, and provider trust can only lower it. Provider placement reads provider trust directly (`src/trust.rs:85-124`). This implements split subject-matter opinions, not a self-certified reputation.

**`[FACT]`** A capability contains issuer, audience, scope, validity, nonce, optional parent proof, and signature. Issuance and verification enforce attenuation and expiry (`src/identity/custody.rs:294-486,681-805`). The default generic leash is broad/90-day with environment tightening; `subject_is_human` bypasses that tightening (`src/identity/custody.rs:177-289`). WG-Exec does not use the broad graph default: it issues two explicit task-scoped capabilities and sets the human flag false (`src/commands/exec_fed_cmd.rs:544-587`).

**`[FACT]`** Review computes exact-byte CID, derives trust/sensitivity depth, normalizes and scans content, conditionally invokes a weak→strong model, and returns strictest-pass verdict (`src/review/mod.rs:326-461`). Candidate evaluation instead binds a task generation, attempt/fence, candidate/manifest/dependency digests, validation result, policy, and exact model route (`src/evaluation/mod.rs:82-217`). They solve different problems: **review is safe-to-consume; evaluation is fit-to-accept for a task**.

### 2.6 Cross-system sequence: human request to learning

**`[INFERENCE]` Local abstract (high confidence).** The intended chain has strong authentication, content, capability, and commit gates, but three transitions remain manual or disconnected: message→graph task, coordinator→provider lifecycle, and accepted modern evaluation→agency learning.

```text
1. HUMAN / LOCAL AUTHORITY
   attended request OR confirmed Telegram sender
      -> local tools/task creation allowed by that surface's contract

2. TASK + PERSONA + MODEL
   task -> optional agency Agent hash
        -> Role/Tradeoff/Components/Outcome prompt
        -> context scope + bound session summary
        -> handler-first exact model route

3. LOCAL CANDIDATE
   worker attempt -> immutable candidate
      -> bounded or deep-readonly evaluation record
      -> exact-candidate accept/reject/finalization
      -X-> no exactly-once projection into agency performance/evolver

4. FEDERATED INBOUND
   relay bytes -> sigchain/signature/recipient authentication
      -> local author trust (peer source; provider may only tighten; revoke override)
      -> IC4 exact-byte review
      -> body exposed only on accept at default `wg msg poll`
      -> operator/external controller creates graph task (not automatic)

5. REMOTE PLACEMENT
   task sensitivity + graph position + provider registry
      -> hard filter + leash decision
      -> signed offer -> signed claim -> signed RunGrant
      -> task-scoped act-as-agent + graph/write UCANs
      -> ContextScope::Task sealed slice + lease epoch

6. REMOTE RUN
   provider verifies grant/caps and opens slice
      -> explicit command OR grant-named model
      -> provider signs ResultEnvelope as delegated producer

7. CANONICAL ACCEPT
   attribution -> graph-write scope -> IC2 review
      -> trust/sensitivity-required disjoint pinned-spec re-run
      -> locked lease-epoch CAS
      -> registry renewal -> best-effort graph accounting
      -> optional task finalization / candidate evaluation

8. LEARNING / HUMAN OVERSIGHT
   review revoke -> local review trust override + named rerun consumers
   provider verify failure -> provider trust lowering
   identity/capability revoke -> separate cryptographic stores
   modern candidate verdict -X-> agency evolution store
   quarantined review item -X-> no shipped human release queue
```

Primary flow evidence: attended contract `src/text/attended_chat_contract.md:1-18`; prompt/persona/memory `src/service/executor.rs:1221-1386`; IC4 authentication/review `src/commands/identity_cmd.rs:1106-1339,1353-1428`; grant `src/commands/exec_fed_cmd.rs:544-650`; run `:700-822`; accept `:851-979`; modern evaluation `src/evaluation/mod.rs:30-235`; legacy learning `src/agency/eval.rs:49-211`.

**`[UNCERTAINTY]`** No repository-owned end-to-end flow was executed by this synthesis. The family smoke source choreographs the full sequence manually, while the newer auto-wire smoke covers only authenticated message review. See `15-review-exec-pilot.md:35-82,157-177,307-326`.

## 3. Findings and enforcement-versus-claim matrix

### 3.1 Findings

#### `XAUTH-001` — WorksGood has a coherent typed authority vector, not a universal trust scalar

- **Label/state:** `[FACT]` + `[INFERENCE]`; shipped core, partially documented
- **Severity/likelihood/confidence:** S4 positive control with S2 conceptual risk; high
- **Affected boundary:** every identity/trust/security claim
- **Evidence:** agency hash, `wgid`, local trust resolver, capability, review, evaluation, and lease are separate types and enforcement sites (`src/agency/hash.rs:15-67`; `src/identity/keys.rs:135-184`; `src/trust.rs:85-124`; `src/identity/custody.rs:294-486`; `src/review/mod.rs:326-461`; `src/evaluation/mod.rs:82-217`; `src/providers/lease.rs:268-341`).
- **Conclusion:** the separation prevents common category errors: signature≠safety, trust≠permission, capability≠correctness, context≠trust, model≠principal.
- **Residual:** comments and product narratives repeatedly compress this into “identity,” “one trust dial,” or “agent,” making wrong joins easy.
- **Recommendation:** `XAUTH-REC-001`.

#### `XAUTH-002` — Candidate and remote-result gates are strongest when bound to immutable evidence and canonical writes

- **Label/state:** `[FACT]`, `[VERIFIED]` (dependency execution); shipped
- **Severity/confidence:** S4 positive control; high
- Modern evaluation records exact source candidate, attempt fence, validation, route, evidence, and one-time consumption (`src/evaluation/mod.rs:82-235`; `13-agency-evaluation-chat.md:181-187`). Deep evaluation denies source/config/graph writes, arbitrary command, network, credentials, authoring identity, and live-worktree reuse (`src/evaluation/deep.rs:60-160`).
- Remote accept authenticates the producer, verifies task-scoped graph-write authority, reviews exact bytes, requires a disjoint re-run for lower-trust/high-sensitivity work, then applies a locked epoch CAS (`src/commands/exec_fed_cmd.rs:851-958`; `src/providers/verify.rs:54-178`; `15-review-exec-pilot.md:129-143`).
- **Bound:** the executable pinned spec is optional; substring checks remain a fallback. The epoch commit is not transactionally joined to graph completion (`XAUTH-010`).

#### `XAUTH-003` — Content trust correctly starts after authentication, but audit and human escalation lag enforcement

- **Label/state:** `[FACT]`; shipped/partial
- **Severity/likelihood/confidence:** S2; possible; high
- IC4 verifies sender/recipient before review and withholds non-accepted bodies (`src/commands/identity_cmd.rs:1106-1223,1353-1428`). Review treats missing provenance as `Unknown`, infers sensitivity upward, and pins the verdict to exact bytes (`src/review/mod.rs:342-461`).
- The verdict log is a local hash-linked JSONL file without signatures or load-time CID/link verification; live callers record best-effort (`src/review/verdict.rs:53-190`; `src/commands/identity_cmd.rs:1292-1333`). Pass 3/human Pass 4 are stubs/deferred (`src/review/depth.rs:29-40,97-104`).
- **Impact:** enforcement can block unsafe bytes, but humans may lack durable evidence and there is no shipped adjudication/release workflow for false positives.
- **Recommendation:** `XAUTH-REC-004`.

#### `XAUTH-004` — same-UID shell authority collapses the claimed federation custody boundary

- **Label/state:** `[FACT]` + `[INFERENCE]`; partial
- **Severity/likelihood/confidence:** **S1 High; likely in same-UID deployments; high**
- **Leaf origin:** adopts `FED-003` and compounds it with `CHAT-001`/worker tool authority.
- `Custodian::sign_digest` loads the seed in-process from the current user's keystore; without a KEK the stored value is plaintext, and the warning is opt-in (`src/identity/keys.rs:223-377`). It has no authenticated requester, purpose, rate-limit, or audit parameters.
- The attended contract permits explicit human-directed read/write/execute/graph/service operations subject only to actual OS/tool restrictions (`src/text/attended_chat_contract.md:1-18`). Ordinary task agents also commonly receive repository/file/shell tools; context isolation is a prompt/data boundary, not an OS principal boundary.
- **`[INFERENCE]`:** if worker and custodian share UID/HOME, compromise of any shell-capable agent or attended session can plausibly bypass the intended “worker never reaches root” separation. UCAN attenuation then protects remote delegation but not theft/use of the principal's local signing keys.
- **Counterevidence:** public bundles contain no private key; API callers receive signatures/shared secrets, not seed bytes. That protects downloaders and other UIDs, not hostile same-UID code.
- **Recommendation:** `XAUTH-REC-002`.

#### `XAUTH-005` — agency persona, cryptographic principal, provider, and human identities are not bound

- **Label/state:** `[FACT]` + `[INFERENCE]`; partial/unknown semantics
- **Severity/likelihood/confidence:** S1; possible; high for missing source binding
- Agency `Agent.id` hashes role/trade-off IDs and is assigned in `Task.agent`; `wgid:` is a genesis key. No inspected structure asserts “this exact agency agent is controlled by this exact `wgid` under this human binding.” Optional signed `AgentFields` are strings and are not a verified import into agency state (`src/agency/hash.rs:56-67`; `src/identity/envelope.rs:44-80`; `src/commands/identity_cmd.rs:344-355`).
- Agency `Agent.trust_level` is serialized but unused by automatic assignment ranking; author/provider trust comes from other local registries (`src/agency/types.rs:500-540`; `src/commands/assign.rs:205-393`; `src/trust.rs:85-124`).
- `wg identity delegate --human` is an issuer/operator boolean that bypasses leash tightening; it is not derived from Telegram confirmation, agency `Agent::is_human`, or signed identity class (`src/commands/identity_cmd.rs:2117-2171`; `src/identity/custody.rs:253-289`).
- **Impact:** UI/prose can imply a continuous “Bruno” while persona choice, key custody, compute provider, model, and human assurance can change independently. An operator mistake can also mark a delegated agent as human and bypass an environment leash.
- **Recommendation:** `XAUTH-REC-003`; human decision `XAUTH-DEC-001`.

#### `XAUTH-006` — accepted modern evaluation does not close the agency learning loop

- **Label/state:** `[FACT]` + `[INFERENCE]`; partial
- **Severity/likelihood/confidence:** **S1; likely; high**
- **Leaf origin:** adopts `AGENCY-004`/`AGENCY-RISK-001`.
- Candidate evaluation persists task `EvaluationRecord`s and finalization evidence. Agency performance/evolution reads the separate `.wg/agency/evaluations` plane; no modern evaluator calls `record_evaluation[_with_inference]` (`src/evaluation/mod.rs:194-235`; `src/agency/eval.rs:49-211`; `src/agency/evolver.rs:110-224`; call-site search in `13-agency-evaluation-chat.md:359-377`).
- **Cross-boundary consequence:** security/evaluation gates can correctly reject candidates while automatic assignment continues ranking on stale or manually populated history. “Verified” provider/author trust and candidate quality also remain intentionally distinct, so no other trust plane repairs this learning starvation.
- **Recommendation:** `XAUTH-REC-005`; human decision `XAUTH-DEC-002`.

#### `XAUTH-007` — local memory and adaptive content bypass the federated content-trust discipline

- **Label/state:** `[FACT]` + `[INFERENCE]`; current/partial
- **Severity/likelihood/confidence:** S2; possible; high
- Bound `session-summary.md` is inserted verbatim into a later worker prompt as “your own memory,” with no content CID, author/model/time provenance, review verdict, or spotlight delimiter (`src/service/executor.rs:1342-1386`). Context scope controls inclusion quantity, not provenance (`src/context_scope.rs:1-70`).
- Function application/memory uses separate JSON/YAML state, and normal apply tracking rows are not the `RunSummary` schema loaded for adaptation (`13-agency-evaluation-chat.md:204-221`). Federated state has a distinct, partial safety gate; it does not govern local session/function memory.
- **`[INFERENCE]`:** content that enters through a less-defended local history path can influence a future worker, become a signed result/event under valid capabilities, and then appear trustworthy to downstream authentication. Review at IC2/IC4 may catch known poison patterns, but provenance has already been lost.
- **Recommendation:** `XAUTH-REC-006`.

#### `XAUTH-008` — remote model intent is signed, but actual silicon/backend provenance is not

- **Label/state:** `[FACT]` + `[INFERENCE]`; partial
- **Severity/likelihood/confidence:** S2; likely whenever command backend is used; high
- A signed `RunGrant.model` names the authorizer-expected model (`src/providers/mod.rs:487-515`). Worker backend precedence allows explicit `--worker-cmd` or `WG_EXEC_WORKER_CMD` to replace the model handler (`src/providers/worker.rs:63-91`).
- `run_worker_run` reports actual backend/model only in local command output; the signed `ResultEnvelope` contains agent, producer, epoch, work product, usage, capabilities and time, but no backend/model/route digest (`src/commands/exec_fed_cmd.rs:748-822`; `src/providers/mod.rs:553-590`).
- **`[INFERENCE]`:** signatures prove which provider returned bytes under which authority, not which model/process produced them. This is acceptable for a trust-and-verify execution protocol only if docs call model an intent/accounting label. It is insufficient for policy requiring a particular model, isolation, or reasoning level.
- **Recommendation:** `XAUTH-REC-007`.

#### `XAUTH-009` — trust demotion and revocation do not compose into one incident response

- **Label/state:** `[FACT]` + `[INFERENCE]`; partial
- **Severity/likelihood/confidence:** S1; possible; high
- Review revoke lowers `review/trust_overrides.json` and names recorded consumers (`src/review/verdict.rs:229-280`). Author trust callers fold that override; provider placement reads provider registry directly (`src/trust.rs:85-124`; `src/providers/placement.rs:142-254`). Provider re-run failure can lower provider trust, while identity key revocation and capability revocation use federation sigchain/revocation structures.
- The review verdict log may be absent because recording is best-effort, so `rerun_consumers` can be incomplete (`RXP-004`). Existing unexpired UCANs are not automatically revoked merely because a review author is demoted.
- **`[INFERENCE]`:** one actor can be “poisoned author,” “Verified provider,” valid cryptographic signer, and holder of live task capabilities simultaneously. The split is correct semantically, but the system lacks a policy-defined correlated response for severe incidents.
- **Recommendation:** `XAUTH-REC-008`; human decision `XAUTH-DEC-003`.

#### `XAUTH-010` — remote execution has a strong protocol boundary but no complete owned lifecycle transaction

- **Label/state:** `[FACT]` + `[INFERENCE]`; partial/manual
- **Severity/likelihood/confidence:** S1; likely for daemon users; high
- Typed `remote_provider` metadata plans `RemoteRunner`, but `spawn_task` rejects it and instructs the caller to use the provider plane (`src/dispatch/plan.rs:583-640`; `src/commands/spawn_task.rs:339-348`). Offer, claim, grant, run, renew, accept and sweep are separately invoked CLI steps.
- Accept saves the lease epoch before provider renewal persistence, graph accounting, and optional terminal finalization (`src/commands/exec_fed_cmd.rs:951-979`). A later failure can leave the result replay-blocked but the task incomplete.
- **Impact:** the secure protocol can work under careful choreography, but the normal coordinator and pilot do not own crash recovery, renewal, or full completion. This is an authority/liveness seam because the canonical fence and graph lifecycle can disagree.
- **Recommendation:** `XAUTH-REC-009`.

### 3.2 Enforcement-versus-claim matrix

| Claim or authority question | Current enforcement | Bypass/counterevidence | State / synthesis judgment |
|---|---|---|---|
| “Agency identity is immutable/content-hashed” | deterministic partial hash equations | prompt-visible fields omitted; included-field edit may delete old file | **Partial/contradicted** — `AGENCY-002` |
| “Agency trust controls selection” | `Agent.trust_level` serialized | auto assignment ranks scoped performance; no read found | **Descriptive/unwired** — `XAUTH-005` |
| “Model selects handler/silicon” | handler-first route; exact route captured for modern evaluation | persona/wgid unchanged on route change; remote command backend can replace model | **Shipped local selection; remote provenance partial** |
| “`wgid:` proves identity” | address-root binding, strict signatures, root-locked chain | first-contact binding/freshness and custody remain operational assumptions | **Shipped authentication, not trust** — `FED-001/002` |
| “Worker never reaches root” | public records omit private keys; custody API returns signatures | same process/UID reads keystore; plaintext fallback; no requester/purpose boundary | **Not enforced against hostile same-UID worker** — `XAUTH-004` |
| “Trust is one dial” | one enum/order; canonical resolver uses peer source and provider min | author/provider are split; review override and crypto/provider revocation are separate | **Narrow to one vocabulary with typed local opinions** — `RXP-002`, `XAUTH-009` |
| “Capability authorizes action” | signature, chain, attenuation, expiry, revocation input, `permits` at relying gate | generic birth default broad/90-day; relying parties must call gate; first-contact revocation TOFU | **Strong core, operational distribution partial** — `FED-011` |
| “Humans are never leashed” | `subject_is_human` bypass in `LeashPolicy::apply` | `--human` is issuer assertion, not bound human identity | **Implemented policy; identity assurance unresolved** |
| “Context scope is least privilege” | ordered clean/task/graph/full; remote task slice and sealing | local scope is quantity, not trust; bound memory inserted separately | **Exposure control, not content trust** — `CONTEXT-001`, `XAUTH-007` |
| “`to` is encryption ACL” | CLI constructs wraps for resolved recipients | library has independent routing `to` and wrap set | **Wrap set is actual ACL** — `FED-010` |
| “Authentication prevents unsafe input” | authentication before IC4 review | valid signer can send malicious content | **False category; review remains required** |
| “All four ingest classes are enforced” | IC1, IC2, IC3, IC4 hooks exist; main paths default-on | IC3 takes manual trust; raw identity poll opt-in; explicit `--no-review`; audit best-effort | **Class coverage shipped; entry-point policy differs** — `RXP-001` |
| “Pass 2 is a quorum / human escalation exists” | deterministic detector and conditional weak→strong model | deterministic `n` ignored; no independent N quorum; Pass 3/4 stubs | **Partial/deferred** — `RXP-003` |
| “Every verdict is on a sigchain” | locked, hash-linked, content-addressed JSONL | unsigned; no link/CID validation on load; live recording errors ignored | **Claim rejected; local best-effort hash chain** — `RXP-004` |
| “Candidate evaluation gates exact work” | source/attempt/route/evidence bound, one-time consume, observation-only deep lane | legacy evaluator remains non-transactional and feeds separate store | **Modern gate strong; dual-plane migration unresolved** — `EVAL-001/002` |
| “Evaluation makes agency adaptive” | legacy manual evaluator updates performance/evolver inputs | modern automatic records are not projected | **Broken composition** — `XAUTH-006` |
| “Remote provider can write only T” | two task-scoped UCANs + accept-time scope check | custody/issuer compromise supersedes delegation; revocation freshness partial | **Strong task blast-radius control** |
| “Remote result proves the named model ran” | signed grant names model; run output displays backend/model | command override; signed result lacks actual backend/model | **Not proven** — `XAUTH-008` |
| “Remote result acceptance is safe and atomic” | auth→scope→review→re-run→epoch CAS | optional substring oracle; post-CAS accounting/finalization gap | **Strong pre-commit gates, incomplete cross-store transaction** |
| “Dispatcher runs remote tasks” | planner selects `RemoteRunner` | spawn path rejects; manual provider choreography | **Planning only, not lifecycle** — `RXP-006` |
| “WG-Fed complete / Pilot turnkey” | substantial protocol and dry-run rehearsal | Proposed ADRs; missing custody/transport controls; real pilot is one-host bootstrap | **Overclaim; experimental/partial boundaries must remain visible** |

### 3.3 Spark and deferred boundary catalog

**`[FACT]`** The following limits must survive every synthesis and product claim:

| Surface | Shipped/verified core | Spark, manual, conditional, or deferred boundary |
|---|---|---|
| Agency | prompt compositions; manual deterministic `assign --auto`; legacy learning store; modern candidate gates | automatic coordinator assignment not found; modern verdict→learning projection absent |
| Federation identity | self-certifying address, sigchain/root lock, signed/sealed envelopes, UCAN core | hostile-worker custody isolation, authenticated compatibility, robust recovery, inbox auth/ack, DHT/Iroh, first-contact witnesses, complete state safety deferred/partial; ADRs remain Proposed |
| Review | deterministic floor, conditional model path, digest pin, IC1–IC4 hooks | independent quorum absent; Pass 3 sandbox and Pass 4 human release deferred; audit log unsigned/best-effort; bypass semantics vary |
| Execution federation | signed protocol, task UCANs, sealed slice, real command/model worker, accept gates, persistent epoch fence | coordinator driver, owned renew/sweep loop, TEE/confidential execution, broad market/quorum deferred; executable re-run optional |
| Family e2e | executable script composes two homes and relay | explicit CLI choreography; full script bypasses auto-review then invokes it manually; not executed in the review leaf environment |
| Pilot | safe-default parser, deterministic dry-run, idempotent down | dry-run worker is a fixed command; no Nora re-run/result return; real `up` is per-host bootstrap and does not wire key/services/live check |
| Human control | attended request contract; confirmed Telegram reply routing | manual confirm lower assurance; no review quarantine queue/release; no binding from human agent to `wgid` or UCAN human flag |

**`[CONTRADICTION]`** “WG-Fed is complete” and “pilot is one-command stand-up” are not adopted. They conflate completed spark slices with production security/operations. Current source authority is also normatively unresolved because all federation ADRs remain Proposed while code shipped (`FED-013`; `RXP-011`).

## 4. Contradictions and drift

**`[INFERENCE]` Local abstract (high confidence).** Most contradictions are not competing implementations of the same rule; they are category collapse or time-layer confusion. This section preserves both sides and the unresolved authority.

| ID | Contradiction | Current synthesis disposition |
|---|---|---|
| `XAUTH-DRIFT-001` | Agency manual describes immutable behavioral identity; hashes omit behavioral fields and edits can delete predecessor (`AGENCY-DRIFT-001`). | **Open.** Source governs current behavior; product must choose mutability/lineage semantics. |
| `XAUTH-DRIFT-002` | Manual/config imply automatic agency assignment; coordinator caller absent and tests encode retired/current stories (`AGENCY-DRIFT-002`). | **Open.** Call it manual deterministic assignment until daemon E2E proves otherwise. |
| `XAUTH-DRIFT-003` | “Evaluation feeds evolution”; current automatic candidate evaluation never enters agency store (`AGENCY-DRIFT-003`). | **Open/material.** Two valid planes do not compose. |
| `XAUTH-DRIFT-004` | “One trust dial” suggests one scalar; source intentionally splits author and provider opinions and folds only downward (`RXP-002`). | **Resolved wording:** one enum/order, multiple typed local assertions. |
| `XAUTH-DRIFT-005` | “Every verdict on same sigchain”; records are unsigned and best-effort (`RXP-DRIFT-003`). | **Open.** Rename or implement cryptographic/transactional audit. |
| `XAUTH-DRIFT-006` | “Diverse quorum”; deterministic count ignored and model path is at most weak→strong (`RXP-DRIFT-004`). | **Open/partial.** Preserve independent quorum as deferred. |
| `XAUTH-DRIFT-007` | Custody ADR describes authenticated ssh-agent boundary; source is same-user in-process key loading (`FED-DRIFT-002`). | **Open/S1.** API minimization is not process isolation. |
| `XAUTH-DRIFT-008` | Recovery described as offline/windowed owner backstop; key is co-located and verifier trusts asserted time (`FED-DRIFT-003`). | **Open/S1.** Do not treat recovery as independent host-compromise control. |
| `XAUTH-DRIFT-009` | “to set is ACL”; actual cryptographic ACL is recipient wrap set (`FED-DRIFT-007`). | **Open wording/invariant.** CLI correlates today; library does not enforce. |
| `XAUTH-DRIFT-010` | “Dispatcher wired”; planner chooses remote, spawn rejects (`RXP-DRIFT-001`). | **Open/S1 workflow.** Say placement metadata wired, runtime manual. |
| `XAUTH-DRIFT-011` | All ingest seams default-on/derived; IC3/manual trust and raw poll/explicit bypass differ (`RXP-DRIFT-002`). | **Narrow:** all classes have hooks; entry points and trust sources differ. |
| `XAUTH-DRIFT-012` | Pilot “live/turnkey/key wired”; real path bootstraps one host and only checks key-path existence (`RXP-DRIFT-006/007`). | **Open/S1 operator impact.** Rename to bootstrap or complete runtime. |
| `XAUTH-DRIFT-013` | Federation ADRs forbid code before Acceptance yet remain Proposed after four waves (`FED-DRIFT-001`). | **Open governance.** Authority is unknown; tests cannot ratify policy. |
| `XAUTH-DRIFT-014` | “Human never leashed” is implemented via an unverified CLI boolean, while human binding is separately confirmed locally. | **New cross-plane contradiction.** Policy intent is clear; subject classification authority is not. |
| `XAUTH-DRIFT-015` | Grant prose says authorizer names silicon; command override is permitted and signed result omits backend/model. | **New cross-plane contradiction.** Model is requested intent unless attested result provenance is added. |

**`[DOC-CLAIM]` Resolved apparent contradiction:** static recipient keys and no offline forward secrecy are explicit design choices, not hidden drift (`14-federation-identity-security.md:269-276`).

**`[FACT]` Resolved apparent contradiction:** a broad generic federation leash and narrow WG-Exec task grants can coexist. The generic default is broad/90-day, while exec explicitly requests two task resources and passes `human=false` (`src/identity/custody.rs:177-289`; `src/commands/exec_fed_cmd.rs:544-587`). The remaining question is whether the generic default is acceptable, not whether exec currently grants `graph://*`.

## 5. Risks and gaps

### 5.1 Ranked seam risks

| Rank / ID | Severity / likelihood | Cross-system seam | Plausible consequence | Existing bound / missing check |
|---:|---|---|---|---|
| 1 `XAUTH-RISK-001` | **S1 / likely where same UID** | shell-capable agent or attended chat ↔ in-process custodian | root/recovery use or theft; forged identity/capability; durable takeover | public bundle redaction helps outsiders only; need hostile-worker OS-boundary test |
| 2 `XAUTH-RISK-002` | **S1 / possible** | agency persona/human label ↔ `wgid`/provider/capability audience | authority attributed to the wrong conceptual actor; leash bypass through mistaken `--human` | cryptographic signatures still identify key; no persona↔principal binding invariant |
| 3 `XAUTH-RISK-003` | **S1 / likely** | candidate gate ↔ agency performance/evolver | quality gates work but assignment/evolution learn nothing or learn only legacy/manual outcomes | exact candidate evidence strong; exactly-once projection absent |
| 4 `XAUTH-RISK-004` | **S1 / possible** | review demotion ↔ provider/key/capability revocation | known poisoner remains eligible compute provider or retains live capability | typed trust split prevents accidental upgrade; coordinated response absent |
| 5 `XAUTH-RISK-005` | **S1 / likely for daemon remote tasks** | remote planner ↔ coordinator/provider lifecycle | ready remote task errors/stalls; leases not renewed/swept; pilot appears up while work cannot flow | manual CLI protocol is strong and scriptable |
| 6 `XAUTH-RISK-006` | **S2 / possible** | local session/function memory ↔ review/provenance | stale or poisoned memory influences signed work and loses origin | later IC2/IC4 detector is a backstop, not origin preservation |
| 7 `XAUTH-RISK-007` | **S2 / likely with command backend** | grant model intent ↔ signed result provenance | policy/accounting claims wrong model or isolation ran | provider/result attribution remains valid; actual silicon not proven |
| 8 `XAUTH-RISK-008` | **S2 / possible** | lease CAS ↔ graph accounting/finalization/evaluation | accepted epoch is consumed but graph task remains incomplete and replay is blocked | persistent fence prevents double commit; recovery transaction absent |
| 9 `XAUTH-RISK-009` | **S2 / possible** | review enforcement ↔ audit/human control | content blocked/consumed without durable tamper-evident record; false quarantine has no release path | consume decision itself is fail-closed at named seams |
| 10 `XAUTH-RISK-010` | **S2 / possible** | explicit bypass flags ↔ operator/automation authority | authenticated but unscreened content reaches a caller that assumes default policy | bypass is visible, not accidental; role restriction/audit not established |
| 11 `XAUTH-RISK-011` | **S2 / possible** | recovery/revocation freshness ↔ delegated authority | stale verifier accepts revoked signer/capability; recovery proof replay/backdating | prior-observation and expiry controls exist; first-contact/currentness weak |
| 12 `XAUTH-RISK-012` | **S2 / likely documentation impact** | spark tests ↔ production/turnkey claims | operators deploy an unauthenticated node/custody or incomplete pilot beyond its validated boundary | leaf audits preserve exact executed/inspected scope |

### 5.2 Evidence gaps and uncertainties

- **`[UNCERTAINTY]`** No hostile same-UID worker test attempted to read/use the real custodian, and this synthesis did not inspect production deployment sandboxing outside the repository. A distinct-UID/container deployment could lower `XAUTH-RISK-001`; it is not encoded as a repository invariant.
- **`[UNCERTAINTY]`** No live model reviewer, evaluator, or remote model backend was invoked by this synthesis. Model quality, credential behavior, and prompt-injection resistance remain environment-dependent.
- **`[UNCERTAINTY]`** No test binds an agency agent, chat session, confirmed human, and `wgid:` into one actor and then proves key rotation/persona evolution preserve the intended relationship.
- **`[UNCERTAINTY]`** No failure-injection test proves recovery after remote epoch save but before graph finalization.
- **`[UNCERTAINTY]`** No incident test starts with a later-discovered poisoned result and proves all affected review trust, provider eligibility, active capabilities, dependent tasks, and cryptographic keys reach a policy-consistent state.
- **`[UNCERTAINTY]`** Federation smokes ran in isolated operator mode in the federation leaf; review/exec/pilot smokes were blocked by worker authority in their leaf. These results are not contradictory because the environments and asserted layers differ.
- **`[FACT]`** Test absence is a gap, not proof that every exploit succeeds.

## 6. Recommendations

### 6.1 Factual synchronization work

1. **`XAUTH-REC-001` — `[RECOMMENDATION]` (P0, architecture/docs; linked `XAUTH-001/005`):** publish the typed authority vector and ban unqualified uses of “identity,” “trust,” “agent,” “verified,” and “authorized.” Name persona, principal, provider, model route, content verdict, candidate verdict, and human binding separately. **Acceptance:** each operator/security journey states the issuing authority, persisted evidence, enforcement site, and what it does not prove.
2. **`XAUTH-REC-010` — `[RECOMMENDATION]` (P0, governance/docs; linked drift 007–013):** synchronize claims immediately: current custody is same-user/in-process; federation ADRs are Proposed; wrap set is ACL; review audit is a local best-effort hash chain; independent quorum/human release/TEE/DHT are deferred; remote lifecycle is manual; real pilot is bootstrap. **Acceptance:** README/help/runbook/ADRs/current capability matrix agree without deleting historical spark memos.
3. **`XAUTH-REC-011` — `[RECOMMENDATION]` (P1, test/docs; linked `XAUTH-RISK-012`):** label evidence as protocol-live, deterministic-fixture, credentialed-model, multi-host, or inspected-only. **Acceptance:** no test-pass statement implies a stronger host/model/network/security boundary than the command exercised.

### 6.2 Implementation and verification work

4. **`XAUTH-REC-002` — `[RECOMMENDATION]` (P0, custody/security; linked `XAUTH-004`):** move root/recovery operations behind a separately authenticated signer principal unavailable to worker UIDs. Bind requester, purpose, identity, digest, capability scope, rate and audit policy; fail closed on plaintext custody for production profiles. **Acceptance:** a hostile shell worker cannot read key material, invoke arbitrary signing, bypass by unsetting environment variables, or access the signer socket outside an authorized purpose.
5. **`XAUTH-REC-003` — `[RECOMMENDATION]` (P0, identity/agency/human; linked `XAUTH-005`):** define a signed local binding record among agency persona ID, `wgid`, human/agent classification, owner, valid-from/head, and allowed model/provider roles. Do not import self-declared `agent_fields.trust_level` as local trust. Derive `--human` semantics from an authorized binding or require an explicit audited override. **Acceptance:** rotation preserves principal continuity; persona evolution creates explicit successor linkage; a different persona/key/human cannot inherit authority silently.
6. **`XAUTH-REC-004` — `[RECOMMENDATION]` (P0, review/audit/human; linked `XAUTH-003/009`):** make verdict persistence required at enforcing consume edges, sign or otherwise tamper-verify the chain, validate CIDs/links on load, persist reviewer source/route/policy, and add a real quarantine/adjudication/release record. **Acceptance:** injected storage failure cannot produce consumed-but-unrecorded bytes; deterministic/weak/strong/human sources are inspectable; release is digest-bound and attributed.
7. **`XAUTH-REC-005` — `[RECOMMENDATION]` (P0, evaluation/agency; linked `XAUTH-006`):** project each consumed modern verdict exactly once into a normalized, context-partitioned agency learning ledger. Superseded/replayed candidates must be inert. **Acceptance:** one accepted/rejected candidate updates its assigned composition once; crash/retry does not duplicate; evolver and assignment consume the same record; legacy migration is explicit.
8. **`XAUTH-REC-006` — `[RECOMMENDATION]` (P1, memory/context/review; linked `XAUTH-007`):** content-address local session/function memories and carry author/session/model/time/parent provenance. Treat memory as data with spotlighting, and route cross-session/cross-principal or externally derived memory through the same review policy. **Acceptance:** mutated memory is detected; its origin is inspectable; an adversarial summary cannot become unlabelled “own memory”; apply→completion→next-apply memory is schema-compatible.
9. **`XAUTH-REC-007` — `[RECOMMENDATION]` (P1, execution/model; linked `XAUTH-008`):** decide whether model is merely requested intent or an enforced requirement. If enforced, include actual backend, exact model/handler/reasoning, route digest, isolation evidence, and usage authority in the signed result; reject unauthorized command substitution. **Acceptance:** a result produced through `--worker-cmd` cannot claim the grant model without an explicit allowed substitution record.
10. **`XAUTH-REC-008` — `[RECOMMENDATION]` (P0, security response; linked `XAUTH-009`):** define severity-based correlated response across review trust override, provider registry, UCAN revocation, identity key status, dependent-task taint, and human notification. Keep the typed distinctions, but drive them from one incident record and fail closed when required evidence is missing. **Acceptance:** a poison-revoke scenario deterministically identifies and fences all affected future actions and names/requeues downstream consumers without upgrading unrelated trust.
11. **`XAUTH-REC-009` — `[RECOMMENDATION]` (P0, coordinator/result lifecycle; linked `XAUTH-010`):** implement an owned remote state machine and recoverable transaction spanning placement, renewal/sweep, result gates, epoch commit, graph accounting/artifact publication, finalization, and candidate evaluation; otherwise reject remote metadata at admission as manual-only. **Acceptance:** a two-home `wg service` test completes without direct provider choreography, survives restart/failure after each phase, and never strands a committed epoch or accepts a stale result.
12. **`XAUTH-REC-012` — `[RECOMMENDATION]` (P1, federation; linked risks 001/011):** carry forward the leaf's custody, recovery, inbox auth/ack, signed compatibility, freshness/revocation-head, wrap-set, and state-safety fixes (`FED-REC-005..012`). **Acceptance:** preserve each adversarial criterion in `14-federation-identity-security.md:307-317`; do not claim completion based only on happy-path federation waves.
13. **`XAUTH-REC-013` — `[RECOMMENDATION]` (P1, policy/bypass):** restrict or audit `--no-review`, raw poll, manual trust, manual human classification, and manual confirmation according to operator role and sensitivity. Add a machine-readable nonzero/semantic rejection contract. **Acceptance:** every bypass emits an attributed durable event; high-value deployment policy can disable it; automation cannot mistake `{accepted:false}` for acceptance.

### 6.3 Human product/design decisions

14. **`XAUTH-DEC-001` — `[RECOMMENDATION]` (P0, product/security; linked `XAUTH-005`):** decide what makes a subject “human” across agency, channel binding, federation identity, and capability policy. Specify assurance for numeric Telegram ID, handle, manual confirmation, and local operator assertion. **Acceptance:** one documented hierarchy determines whether leash bypass/high-impact approval is allowed.
15. **`XAUTH-DEC-002` — `[RECOMMENDATION]` (P0, agency/product; linked `XAUTH-006`):** decide whether performance is immutable identity content, mutable contextual evidence, or an external ledger. **Acceptance:** hash/lineage, assignment, evaluation projection, and evolution implement one choice without deleting historical evidence.
16. **`XAUTH-DEC-003` — `[RECOMMENDATION]` (P0, security/product; linked `XAUTH-009`):** define relationships among content mistrust, provider mistrust, key compromise, and capability revocation. A content false positive should not destroy an identity, while a confirmed malicious provider may require all four responses. **Acceptance:** a policy matrix names triggers, automatic actions, human approvals, rollback, and evidence.
17. **`XAUTH-DEC-004` — `[RECOMMENDATION]` (P1, identity/privacy):** decide historical signature semantics, first-contact freshness/revocation requirements, default message sealing, and whether broad 90-day generic capabilities remain acceptable. **Acceptance:** ADR, verifier, CLI defaults, and operator warnings agree.

## 7. Evidence appendix

### 7.1 Snapshot, method, and dependency evidence

**`[FACT]`** The normative audit charter is [`README.md`](README.md), which requires labels, seven sections, direct primary evidence, stable dependency IDs, explicit uncertainty, and preservation of contradictions (`README.md:172-338`). This synthesis read the charter and all three dependency artifacts in full:

- [`13-agency-evaluation-chat.md`](13-agency-evaluation-chat.md), 420 lines;
- [`14-federation-identity-security.md`](14-federation-identity-security.md), 473 lines;
- [`15-review-exec-pilot.md`](15-review-exec-pilot.md), 353 lines.

**`[FACT]`** The dependency execution records, not this synthesis, support `[VERIFIED]` behavior claims:

- agency/evaluation/chat/context commands and outcomes: `13-agency-evaluation-chat.md:329-416`;
- build, 100 identity tests, and four operator-mode federation smokes: `14-federation-identity-security.md:325-473`;
- review/provider/trust/pilot/planner tests and worker-authority-blocked smoke attempts: `15-review-exec-pilot.md:272-353`.

**`[UNCERTAINTY]`** Their differing smoke outcomes reflect different execution modes. The federation leaf explicitly removed inherited worker-control variables and limited its claim to disposable operator-mode protocol behavior. The review/exec leaf retained the worker boundary and could not execute graph mutations. Neither result verifies production worker governance.

### 7.2 Primary source spot-check index

| Topic | Direct evidence checked by this synthesis | What it supports |
|---|---|---|
| persona/hash/trust fields | `src/agency/hash.rs:15-67`; `src/agency/types.rs:328-554`; `src/commands/assign.rs:205-393` | agency identity equations; serialized but selection-unread trust; performance ranking |
| model routing/agency one-shots | `src/dispatch/handler_for_model.rs:1-137`; `src/service/llm.rs:252-345` | route selects handler; role-specific model authority/fallback constraints |
| prompt/context/memory | `src/context_scope.rs:1-70`; `src/service/executor.rs:1221-1386`; `src/text/attended_chat_contract.md:1-18` | context quantity, persona rendering, unscreened bound summary, attended authority |
| candidate evaluation | `src/evaluation/mod.rs:30-235`; `src/evaluation/deep.rs:60-160` | exact source/route/evidence record and observation-only deep capabilities |
| human binding | `src/agency/human_binding.rs:38-269` | numeric/handle matching, confirmation, local routing authority |
| identity/custody | `src/identity/keys.rs:215-398`; `src/identity/envelope.rs:44-153`; `src/identity/sigchain.rs:680-886` | in-process keystore use, public agent metadata, self-certifying/root-locked identity |
| generic capability/leash | `src/identity/custody.rs:177-317,403-486,681-805`; `src/commands/identity_cmd.rs:2103-2240` | broad default, human bypass, issue/verify, CLI assertion |
| trust/review | `src/trust.rs:1-124`; `src/review/depth.rs:1-104`; `src/review/mod.rs:246-461`; `src/review/verdict.rs:53-280` | split opinions, depth matrix/stubs, exact-byte gate, unsigned/best-effort log and override |
| IC4 edge | `src/commands/identity_cmd.rs:1106-1339,1353-1428` | auth before review, body withholding, explicit raw bypass, best-effort record |
| provider placement/grant | `src/providers/placement.rs:126-254`; `src/providers/mod.rs:430-625`; `src/commands/exec_fed_cmd.rs:544-650` | fail-closed leash, protocol envelopes, two task capabilities and sealed slice |
| provider backend/result | `src/providers/worker.rs:50-199`; `src/commands/exec_fed_cmd.rs:700-822`; `src/providers/mod.rs:553-590` | command/model precedence; actual route absent from signed result |
| result acceptance | `src/providers/verify.rs:54-178`; `src/commands/exec_fed_cmd.rs:851-979`; `src/providers/lease.rs:268-341,404-500` | attribution/scope, review/rerun/CAS ordering, post-CAS gap |
| remote lifecycle | `src/dispatch/plan.rs:583-640`; `src/commands/spawn_task.rs:339-348` | planner selection and rejecting local spawn seam |

### 7.3 Static commands executed by this synthesis

**`[VERIFIED]`** The following are static repository observations only, executed in the task worktree on 2026-08-08. They do not establish runtime reachability:

```bash
rg -n "trust_level" src/agency src/commands/assign.rs \
  src/commands/service/assignment.rs src/service
# production semantic occurrences were the Agent field/default construction;
# automatic assignment selection did not read the field

rg -n "agent_fields|AgentFields" src/identity src/commands
# type, identity mint, node/test construction; no agency binding consumer found

rg -n "remote_provider|RemoteRunner" src/service src/dispatch src/commands
# planner creates RemoteRunner; spawn_task rejects it; provider CLI is the driver

rg -n "record_evaluation_with_inference|record_evaluation\\(" src --glob '*.rs'
# legacy/manual agency paths; no src/evaluation/{mod,bounded,deep}.rs projection

rg -n "subject_is_human|issue_root\\(" src/identity src/commands
# generic leash boolean and explicit exec grants with human=false
```

**`[UNCERTAINTY]`** Call-site absence is strong E2 evidence inside this repository, not proof that an external deployment wrapper or dynamically invoked command does not supply a missing join.

### 7.4 Validation commands

**`[FACT]`** The final task validation is recorded by exact commands and exit status in the task log. Required checks:

```bash
test -s docs/audit/2026-08-08-worksgood-system/21-agency-federation-safety-synthesis.md
git diff --check
```

This is an audit-only Markdown change, so no production build/test claim is made by this synthesis.

### 7.5 Limitations

- **`[FACT]`** No production source, tests, pre-existing docs, identities, keys, provider registries, or graphs were modified.
- **`[FACT]`** No cargo test, smoke, model call, network flow, Telegram flow, TUI flow, provider run, recovery, or destructive identity action was executed for this synthesis.
- **`[UNCERTAINTY]`** This is not a cryptographic or production-readiness certification. It relies on leaf execution evidence for bounded behaviors and directly inspected E2 source for synthesis claims.
- **`[UNCERTAINTY]`** The repository does not reveal all deployment controls. Distinct UIDs, containers, external HSMs, TLS proxies, or policy wrappers outside the repository could mitigate some risks; they are not current repository-enforced invariants.
- **`[INFERENCE]` (high confidence)** The audit's central result remains robust under those uncertainties: WorksGood's authority model is a vector of independently necessary proofs and local policy decisions. The main safety work is to preserve those distinctions while making their cross-plane bindings explicit, transactional, and observable.
