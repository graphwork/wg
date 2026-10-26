# Review, execution federation, trust composition, and pilot audit

**Audit date:** 2026-08-08

**Audit snapshot:** `b0892ea7496fd2cc8f641417a3d8e33ca9add369` (production tree); the audit branch was `98b319c36aa8a21fd4506fc7469fe6d58978cdda`, whose only delta from the snapshot before this artifact was the audit charter

**Evidence checked through:** 2026-08-08

**Freshness:** snapshot-current for source and tests inspected below

**Scope:** WG-Review, WG-Exec, trust resolution, ingest auto-wiring, family-team composition, and WG-Pilot

**Change boundary:** audit artifact only

## 1. Executive abstract

**`[FACT]`** The implementation is materially ahead of the older “spark only” narrative. Review has a deterministic decoding/detection floor and a conditional weak-to-strong model path; trace import (IC1), provider-result accept (IC2), state load (IC3), and federated message poll (IC4) all have consumption-edge gates. WG-Exec has signed offer/claim/grant/result envelopes, two task-scoped capabilities, sealed task slices, a real command/model worker, a locked persistent epoch fence, signed renewal verbs, default-on artifact review, and executable pinned-spec verification. Primary enforcement sites are `src/review/mod.rs:326-461`, `src/commands/trace_import.rs:76-134`, `src/commands/exec_fed_cmd.rs:851-983`, `src/commands/identity_cmd.rs:1106-1339`, `src/commands/identity_cmd.rs:1842-1997`, and `src/providers/{placement,lease,verify,worker}.rs`.

**`[VERIFIED]`** On 2026-08-08, against a build from the pinned production tree, focused unit suites passed: review **53/53**, providers **54/54**, trust **7/7**, pilot parsing/safe defaults **8/8**, and the remote-placement planner test **1/1**. These verify pure/library and CLI-module properties, not a live remote deployment. Exact commands are in section 7.

**`[UNCERTAINTY]`** Five relevant smoke scripts were also attempted with `target/debug/wg`, but the worker harness refused their graph-mutating commands with `worker_control.operation_refused: this command requires operator/graph authority`. Each scenario therefore exited 1 at its first mutation. This is an audit-environment limitation, **not** evidence that the scenario's product assertion failed or passed. Their committed assertions are cited as E3 (inspected, not executed here).

**`[INFERENCE]`** The strongest positive result is the canonical accept boundary: authentication, scoped-write authorization, default-on IC2 review, trust-proportional re-run, and the locked epoch compare-and-set occur before a result is committed (`src/commands/exec_fed_cmd.rs:851-958`). The highest integration risk is that the normal coordinator does not drive that sequence: planning converts typed `remote_provider` metadata to `RemoteRunner`, but the local spawn path returns an error instructing the operator to use `wg provider …`; the only driver found is the manual `wg provider place`/offer flow (`src/dispatch/plan.rs:583-640`; `src/commands/spawn_task.rs:339-348`; `src/commands/exec_fed_cmd.rs:289-346`). Confidence: high from exhaustive `RemoteRunner` references, although no daemon flow could be executed here.

**`[INFERENCE]`** WG-Pilot's dry-run is a useful self-contained rehearsal, but its real-host path is infrastructure bootstrap rather than turnkey end-to-end operation. The dry-run uses a constant shell worker, skips Nora's disjoint verification, and does not send the completion back to Sara; the real path starts one node, mints local identities, registers peers, and records `check_passed: None`. It merely checks whether the configured OpenRouter key path exists and does not load or export the key (`src/commands/pilot_cmd.rs:43-50`, `801-1042`, `1066-1215`). The next decision should be to narrow operator claims immediately, then either wire coordinator-driven remote execution and real two-host pilot checks or explicitly keep both as operator-driven experimental surfaces.

## 2. Scope and map

### 2.1 Inspected surfaces and exclusions

**`[FACT]`** Static inspection covered `src/review/`, `src/providers/`, `src/trust.rs`, `src/commands/{review_cmd,identity_cmd,msg,trace_import,exec_fed_cmd,pilot_cmd,spawn_task}.rs`, `src/dispatch/plan.rs`, `pilot.example.toml`, the five named smoke scenarios, their manifest entries, the content-safety and execution-federation decision memos, `docs/prod-audit/01-production-readiness-followup.md`, `docs/prod-audit/02-live-reviewer-eval.md`, and `docs/ops/runbook.md` section 6.

**`[UNCERTAINTY]`** This leaf audit did not independently review WG-Fed cryptographic primitives, secret backends, network-server hardening, all graph finalization semantics, model-provider routing, notification security, or the full CI matrix. Those belong to sibling audits. It traces how this scope calls those boundaries and does not certify their internals.

### 2.2 Continuous cross-subsystem sequence

**`[FACT]`** The implemented intended sequence is:

```text
untrusted relay bytes
  -> SignedEvent parse + claimed-sender sigchain resolution
  -> signature / address / recipient / optional freshness verification
  -> replay key check
  -> local trust resolution
       peer author trust (source; absent => Unknown)
       MIN provider opinion (may only tighten)
       MIN revoke override
  -> IC4 review (unlabeled => high/deep)
       Pass 0 provenance
       Pass 1 normalize/lint
       Pass 2 deterministic detector OR conditional weak->strong model
  -> accept: expose body; non-accept/replay: body=null + withheld
  -> operator/coordinator creates or selects a graph task
  -> provider registry hard filter + leash
  -> signed offer -> signed claim -> signed RunGrant
       act-as-agent@agent://G/task/T
       graph/write@graph://task/T
       sealed ContextScope::Task bundle
       signed lease(epoch, term, cadence)
  -> provider verifies grant/capabilities, opens only its slice, runs command/model
  -> provider signs ResultEnvelope with usage and both capability proofs
  -> authorizer accept boundary
       attribution -> graph/write authorization -> IC2 review
       -> required disjoint pinned-spec re-run -> epoch CAS
       -> accounting/finalization
```

**`[FACT]`** Authentication precedes review: `authenticate_event` verifies the ordinary or sealed-sender signature and recipient before returning the body (`src/commands/identity_cmd.rs:1353-1428`); only the success branch invokes `review_inbound_event` (`src/commands/identity_cmd.rs:1140-1208`). A forged sender therefore does not become review input.

**`[FACT]`** Trust is local policy, not identity evidence. `resolve_author_trust` sources author trust from a peer record, defaults absence/bare enrollment to `Unknown`, and folds any provider opinion with least-trusting/min semantics (`src/trust.rs:85-124`). A `Verified` provider alone cannot upgrade author trust; the executed trust suite pins this split.

**`[FACT]`** On `wg msg poll --as`, review is on by default and `--review` is now redundant; `--no-review` exposes authenticated bodies unscreened (`src/main.rs:2107-2121`; `src/cli.rs:4636-4649`). The lower-level `wg identity poll` remains opt-in through its `--review` flag (`src/cli.rs:3762-3771`). When enabled, a non-accept verdict or replay produces `body: null`, `body_withheld: true`, and `consumable: false` (`src/commands/identity_cmd.rs:1165-1223`).

**`[FACT]`** The bridge from reviewed message to execution is not automatic. `run_poll` returns JSON/body; it does not create a graph task or a provider offer. The family script writes/passes the accepted text onward itself. The original composition test deliberately polls with `--no-review`, then calls `wg review check --trust …` manually (`tests/smoke/scenarios/e2e_family_team.sh:220-292` [inspected, not run]). The newer auto-wire test removes only that trust/review glue at IC4 (`tests/smoke/scenarios/e2e_autowire_ingest_gate.sh:1-251` [inspected, not run]); it does not auto-place the resulting task.

**`[FACT]`** Placement itself is strongly constrained. `leash` refuses unlabeled work, refuses confidential work without attestation, selects `ContextScope::Task`, couples delegation TTL with lease terms, and chooses verification depth (`src/providers/placement.rs:142-254`). `evaluate_placement` then checks capability, isolation, trust floor, and B-tier checkability (`src/providers/placement.rs:306-421`). Foundational tasks raise the floor to `Verified`, and grant refuses unverified lower-trust upstream inputs (`src/providers/cross_task.rs:31-89`; `src/commands/exec_fed_cmd.rs:489-541`).

**`[FACT]`** Grant issues exactly the two task-scoped UCANs, seals task input plus explicit dependency artifacts to the provider, and persists lease terms (`src/commands/exec_fed_cmd.rs:544-650`). The worker verifies the grant and both UCANs, verifies and opens the sealed bundle, then drives a real command or model backend; no built-in constant-diff fallback exists (`src/commands/exec_fed_cmd.rs:700-822`; `src/providers/worker.rs:50-199`).

**`[FACT]`** Result acceptance orders its gates before the epoch commit: attribution, scoped graph-write authorization, IC2 review, and (for re-run depth) pinned-spec verification all precede `try_commit` (`src/commands/exec_fed_cmd.rs:851-958`). `verify_result` refuses same-provider verification, excludes provider-authored test changes, supports an executable authorizer-owned acceptance command, falls back to a substring oracle when no executable check is present, and applies the shared deterministic poison screen (`src/providers/verify.rs:155-365`, `439-536`).

**`[FACT]`** The persistent fence rejects future/stale epochs and same-epoch replay, and mutations hold an advisory lock across load-modify-save; corrupt present state is refused instead of reset (`src/providers/lease.rs:268-341`, `404-486`). Renewal and timeout sweep exist as explicit CLI verbs (`src/commands/exec_fed_cmd.rs:1237-1384`). No service/coordinator caller of provider `sweep` or `accept-renewal` was found; the operator or an external scheduler must run them.

### 2.3 Trust and authority matrix

| Input/authority | Local source | What it may decide | What it does **not** prove | Enforcement site | Status |
|---|---|---|---|---|---|
| Sender identity | `wgid:` + verified sigchain/signature | Who authored an event; recipient membership | Safety, honesty, or provider trust | `identity_cmd.rs:1353-1428` | **`[FACT]` shipped** |
| Author trust | `federation.yaml` peer `trust` | IC1/IC4 review depth | Compute-box integrity | `trust.rs:85-124` | **`[FACT]` shipped** |
| Provider trust | `exec/registry.json` | Placement floor/leash and IC2 depth | Authorship trust; it can only lower author trust in the resolver | `providers/mod.rs:303-423`; `trust.rs:101-124` | **`[FACT]` shipped** |
| Revoke override | `review/trust_overrides.json` | Tighten the next review | Revocation of cryptographic keys | `review/verdict.rs:229-280`; callers at `identity_cmd.rs:1300-1310` | **`[FACT]` shipped, separate side ledger** |
| Sensitivity | explicit/inferred; absent is unlabeled | Review depth and exec floor/seal | Truth of a self-label | `review/mod.rs:364-381`; `providers/placement.rs:153-183` | **`[FACT]` shipped/fail-closed** |
| Reviewer verdict | deterministic/model classification | Permit or withhold exact content | Correctness/certification; no graph-write authority | `review/mod.rs:342-461`; `pass2_review.rs:50-101` | **`[FACT]` shipped; model conditional** |
| Provider capability ad | provider signature, authorizer registry | Model/isolation eligibility | Trust; attestation is not real in v1 | `providers/mod.rs:286-341`; `placement.rs:354-388` | **`[FACT]` shipped ad; TEE deferred** |
| Act-as-agent UCAN | principal-issued, expiring, task-bound | Attribute P acting as G for T | Artifact correctness | `exec_fed_cmd.rs:544-587`; `verify.rs:54-109` | **`[FACT]` shipped** |
| Graph-write UCAN | principal-issued, expiring, `graph://task/T` | Authorize one task write | Acceptance, epoch freshness, other tasks | `exec_fed_cmd.rs:556-587`; `verify.rs:116-145` | **`[FACT]` shipped** |
| Lease epoch | authorizer ledger | Current placement and one commit | Work correctness | `lease.rs:268-341`, `431-486` | **`[VERIFIED]` focused tests passed** |
| Pinned spec/verifier | authorizer-owned command/fixtures; disjoint verifier id | Integrity/equivalence for checkable output | General semantic correctness outside the spec | `verify.rs:155-365`, `470-536` | **`[VERIFIED]` executable and fallback tests passed** |
| Human | none in review types/CLI | Nothing today | Quarantine release or adjudication | `review/reviewer.rs:496-513`; no release command | **`[FACT]` deferred** |

## 3. Findings

### `RXP-001` — Four ingest classes are reachable, but their trust and bypass semantics differ

**`[FACT]`** **State: shipped/partial; severity S2; confidence high.** IC1 trace import is enforcing by default and omits non-accept tasks (`src/commands/trace_import.rs:76-134`; dispatch passes `!no_review` at `src/main.rs:1781-1792`). IC2 provider accept is enforcing by default (`src/main.rs:4574-4589`; `src/commands/exec_fed_cmd.rs:887-899`). IC4 `wg msg poll --as` is enforcing by default (`src/main.rs:2107-2121`). IC3 has the older state-safety gate and now really persists accepted transparent state (`src/commands/identity_cmd.rs:1842-1997`).

**`[FACT]`** IC3 still accepts an operator-supplied `--author-trust`; it does not call `resolve_author_trust` (`src/commands/identity_cmd.rs:1842-1861`). Raw `identity poll` is opt-in, while `msg poll` and the other new ingest paths have explicit `--no-review` bypasses. Thus “all four are wired” is accurate at the class level, but “all derive trust and enforce by default on every entry point” is not.

### `RXP-002` — Trust is intentionally split, not literally one scalar

**`[VERIFIED]`** **State: shipped; severity S4 positive control; confidence high.** `cargo test --lib trust::` passed 7/7. The source distinguishes author trust from provider trust, then uses the provider opinion only as a minimum/tightener for author review (`src/trust.rs:1-47`, `85-124`). This closes the dangerous upgrade in which enrollment as a trusted box would also make its authored messages trusted.

**`[INFERENCE]`** “One trust dial” is useful shorthand for one enum/order and coordinated local assertions, but it is not one value. Review IC4 uses peer-derived author trust; IC2 and the exec leash use provider trust directly. Documentation should say “one trust vocabulary with split author/provider assertions and monotone composition.”

### `RXP-003` — The review pipeline has real model wiring, but deterministic smoke and quorum claims need qualification

**`[VERIFIED]`** **State: shipped/partial; severity S2; confidence high.** Review tests passed 53/53, including decoding evasions, bounded reasons, weak-to-strong escalation with fake LLMs, fail-closed errors, digest pinning, and revoke. `review_inbound_ctx` uses the model only when `model_review_available`; otherwise it uses the deterministic detector (`src/review/mod.rs:326-442`; `src/review/reviewer.rs:423-441`). The live eval is separately model-gated and refuses a required but unavailable model (`src/commands/review_cmd.rs:142-210`).

**`[FACT]`** The deterministic Pass-2 API accepts `n`, but immediately ignores it; one detector pass supplies the result (`src/review/pass2_review.rs:86-99`). Weak-to-strong model escalation exists, but a genuinely independent N-model quorum does not. Pass 3 is labeled a stub and no human/pending state exists (`src/review/depth.rs:29-40`; `src/review/reviewer.rs:496-513`).

**`[UNCERTAINTY]`** `PipelineOutcome` and `VerdictRecord` do not retain `ReviewSource` or the escalation flag, so an operator reading the ordinary ingest verdict chain cannot tell whether a given decision came from deterministic fallback, weak model, strong model, or fail-closed model failure. The dedicated eval can report source, but the live consumption record cannot (`src/review/mod.rs:270-305`; `src/review/verdict.rs:53-80`).

### `RXP-004` — Review enforcement is fail-closed, but audit recording is best-effort and the “sigchain” is unsigned

**`[FACT]`** **State: partial; severity S2; likelihood possible; confidence high.** IC1 import, IC2 accept, and IC4 message gates ignore errors from `VerdictStore::record` (`src/commands/trace_import.rs:116-118`; `src/commands/exec_fed_cmd.rs:1163-1169`; `src/commands/identity_cmd.rs:1330-1333`). The content decision can still block, but the stated audit leg can silently disappear on disk/lock/serialization failure.

**`[FACT]`** `VerdictRecord` has `prev` and `cid` but no signature; `load_chain` parses records without recomputing CIDs or links (`src/review/verdict.rs:53-80`, `117-190`). It is a locked, hash-linked, content-addressed local log, not a WG-Fed cryptographically signed sigchain. Focused tests verify link construction and concurrent serialization, not tamper verification.

### `RXP-005` — The WG-Exec CLI accept boundary is coherent and substantially hardened

**`[VERIFIED]`** **State: shipped; severity S4 positive control; confidence high.** Provider tests passed 54/54. They include capability filtering, task-slice ACL, task-scoped write, real subprocess worker, nonconstant usage, executable pinned tests, same-provider refusal, replay/stale fencing, corrupt-ledger refusal, and concurrent writers.

**`[FACT]`** Accept runs attribution, write-scope authorization, default-on IC2 review, required low-trust re-run, then the locked epoch CAS (`src/commands/exec_fed_cmd.rs:851-958`). Low-trust acceptance without a pinned spec returns `verification-required`; a failed re-run lowers provider trust and does not consume the epoch (`src/commands/exec_fed_cmd.rs:1016-1087`). This is stronger than the original spark's separate manual `verify` command.

**`[FACT]`** The executable acceptance check is optional. If absent from the pinned spec, verification uses required/forbidden substrings, supplemented by the shared deterministic poison detector (`src/providers/verify.rs:155-178`, `268-333`, `515-533`). “Real executable re-run” is therefore a supported mode, not a universal property of every accepted result.

### `RXP-006` — Remote planning stops before coordinator-driven execution

**`[FACT]`** **State: partial/manual seam; severity S1; likelihood likely for users expecting daemon dispatch; confidence high.** Typed `Task.remote_provider` becomes `Placement::Provider` and `ExecutorKind::RemoteRunner` (`src/dispatch/plan.rs:583-640`), and that planner test passed. But `spawn_task` rejects `RemoteRunner` and tells the caller that the providers plane must drive placement/grant/run/accept (`src/commands/spawn_task.rs:339-348`). Repository-wide search found no service-side `RemoteRunner` implementation.

**`[FACT]`** `wg provider place` only reads task metadata and calls `run_offer`; claim, grant, run, renew, accept, and finalize remain separate invocations (`src/commands/exec_fed_cmd.rs:289-346`). The execution smoke and family e2e scripts explicitly sequence every verb (`tests/smoke/scenarios/exec_spark_borrowed_box.sh:111-388`; `tests/smoke/scenarios/e2e_family_team.sh:304-478` [inspected, not run]).

**`[INFERENCE]`** The provider protocol is runnable, but the ordinary coordinator cannot currently take a ready remote task to completion without an external controller/operator. This contradicts “dispatcher wired” if that phrase is read as an end-to-end runtime path.

### `RXP-007` — Lease fencing is durable; liveness scheduling is manual

**`[VERIFIED]`** **State: shipped/partial; severity S2; confidence high.** Provider tests verify atomic save, lock serialization, refusal on corrupt JSON, timeout sweep, renewal refresh, replay, and stale epochs. `LeaseLedger::open_locked` holds a sidecar lock over load/mutate/save (`src/providers/lease.rs:404-500`).

**`[FACT]`** `wg provider renew`, `accept-renewal`, and `sweep` are reachable CLI operations (`src/commands/exec_fed_cmd.rs:1237-1384`; `src/main.rs:4601-4626`). No coordinator timer calling them was found. A provider deployment needs an external heartbeat/sweep loop; otherwise leases do not auto-renew or auto-reclaim merely because the service runs.

### `RXP-008` — Post-fence integration can strand an accepted result

**`[FACT]`** **State: shipped with gap; severity S2; likelihood possible; confidence high.** The epoch is committed and saved before provider-registry persistence, graph accounting, and optional task finalization (`src/commands/exec_fed_cmd.rs:951-979`). Accounting is explicitly best-effort; if the task is absent or graph modification fails, `accounted` is false. With `--complete-task`, finalization runs only after accounting and may return an error after the epoch is already committed (`src/commands/exec_fed_cmd.rs:966-976`).

**`[INFERENCE]`** A crash or graph-save/finalization failure after lease commit can leave a result replay-blocked but not reflected as completed in the graph. Recovery semantics for this cross-store transaction were not found in this scope. A falsifying check would inject failure after `guard.save()` and prove a reconciliation command can finish idempotently.

### `RXP-009` — Family-team e2e proves composition through explicit CLI choreography, not production automation

**`[FACT]`** **State: executable specification; severity S4; confidence high about test shape, behavior unverified here.** `e2e_family_team.sh` uses two isolated homes/graphs and an HTTP relay, authenticates messages, manually reviews them, manually sequences provider operations, tests wrong-task/expiry/replay/stale behavior, runs disjoint verification, refuses confidential placement, and sends a signed completion back to Sara (`tests/smoke/scenarios/e2e_family_team.sh:1-504` [inspected, not run]).

**`[FACT]`** The script intentionally bypasses the default IC4 gate (`--no-review`) and then hand-passes `verified`/`unknown` to `wg review check` (`tests/smoke/scenarios/e2e_family_team.sh:220-292`). `e2e_autowire_ingest_gate.sh` verifies the later IC4 improvement, but only through poll/review/audit, not the subsequent placement and return path (`tests/smoke/scenarios/e2e_autowire_ingest_gate.sh:128-251` [inspected, not run]).

### `RXP-010` — Pilot safe defaults are validated before startup, but several are declarations rather than installed runtime policy

**`[VERIFIED]`** **State: shipped/partial; severity S2; confidence high.** Pilot tests passed 8/8. `resolve_safe_defaults` refuses review gate values other than `enforcing`, confidential behavior other than `refuse`, discovery other than `configured`, and nonpositive TTL; unsafe validation occurs before node startup (`src/commands/pilot_cmd.rs:181-228`, `483-509`). Dry-run state persists early enough for teardown, and down is idempotent in source (`src/commands/pilot_cmd.rs:550-690`, `1294-1354`).

**`[FACT]`** The enforcement behind three declarations is mostly substrate behavior: default-on review, configured peers, and confidential refusal. `leash_max_ttl_secs` is only passed as `WG_FED_LEASH_MAX_TTL_SECS` to the dry-run grant (`src/commands/pilot_cmd.rs:929-941`). The real-host bring-up neither exports nor persists that environment policy for future provider processes (`src/commands/pilot_cmd.rs:1070-1215`). Any positive integer is accepted, so “bounded” has no maximum policy bound.

**`[FACT]`** Split trust is reported as always true, but configured `[[peers]]` can supply arbitrary trust and `verified_peers` is operator-editable (`src/commands/pilot_cmd.rs:122-168`, `529-546`, `1129-1143`). This is legitimate local authority, but state output proves what the config declared, not that only the four family identities received `Verified`.

### `RXP-011` — Pilot dry-run and real-host claims exceed the implementation

**`[FACT]`** **State: partial/documented-only claims; severity S1; likelihood possible; confidence high.** The dry-run's worker is the constant `DRY_RUN_WORKER_CMD` (`src/commands/pilot_cmd.rs:43-50`, `961-979`). Its live check authenticates/reviews two inbound messages, runs one remote provider flow, rejects a forged result, and refuses confidential placement. It does **not** exercise the Nora verifier (`let _ = nora`) and does not send the signed completion back to Sara (`src/commands/pilot_cmd.rs:801-1042`).

**`[FACT]`** The real path starts only the current host, mints its two identities, wires configured peers, probes endpoints non-fatally, and saves `check_passed: None` (`src/commands/pilot_cmd.rs:1066-1215`). It explicitly says the full check needs both hosts. The OpenRouter key path is only tested with `Path::exists`; no bytes are read and no `OPENROUTER_API_KEY`/secret reference is configured (`src/commands/pilot_cmd.rs:1145-1159`). Telegram entries are written but listeners are not started (`src/commands/pilot_cmd.rs:639-650`, `744-770`).

**`[INFERENCE]`** `wg pilot up --config` is a per-host identity/node/peer bootstrap. It is not yet one-command stand-up of a working family team with running agent services, live reviewer/worker credentials, lease loops, or an end-to-end production check.

### 3.12 Capability state catalog

| Capability | State at snapshot | Evidence / qualification |
|---|---|---|
| Deterministic normalize/decode/detect review | **shipped** | `src/review/{pass1_lint,detect}.rs`; 53 review tests passed |
| Conditional weak-to-strong model reviewer | **shipped, environment-dependent** | `reviewer.rs:277-374`, `423-441`; orchestration unit-tested, no live call here |
| Independent N-model review quorum | **stubbed/partial** | deterministic `n` ignored at `pass2_review.rs:86-99`; one weak + one strong opinion is not N-independent quorum |
| Pass 3 sandbox detonation | **stubbed/deferred** | `depth.rs:29-40`; no execution path |
| Pass 4 human quarantine release | **deferred** | no pending/release type or command; `reviewer.rs:496-513` |
| IC1 trace-import gate | **shipped, default-on, bypassable** | `trace_import.rs:76-134`; `--no-review` |
| IC2 result-accept gate | **shipped, default-on, bypassable** | `exec_fed_cmd.rs:887-899`; `--no-review` |
| IC3 state gate/real transparent consumer | **shipped, legacy/manual-trust** | `identity_cmd.rs:1842-1997`; operator supplies trust |
| IC4 message gate | **shipped default-on on `msg`, opt-in on raw `identity`** | `main.rs:2107-2121`; `cli.rs:3762-3771` |
| Review audit/revoke | **shipped but unsigned/best-effort at live seams** | `review/verdict.rs`; ignored record errors at callers |
| Offer/claim/grant/result protocol | **shipped CLI protocol** | `providers/mod.rs:430-625`; `exec_fed_cmd.rs` |
| Real remote worker backend | **shipped, requires command or credentials** | `providers/worker.rs:50-199` |
| Executable pinned-spec re-run | **shipped optional mode** | `providers/verify.rs:197-365`; substring fallback remains |
| Persistent epoch fence | **shipped** | `providers/lease.rs:268-341`, `404-500`; focused tests passed |
| Lease heartbeat/timeout runtime | **CLI shipped; service automation manual/deferred** | `exec_fed_cmd.rs:1237-1384`; no coordinator caller found |
| Remote planner metadata | **shipped** | `dispatch/plan.rs:583-640`; planner test passed |
| Coordinator-driven remote lifecycle | **documented/partial, not found** | `spawn_task.rs:339-348` rejects; manual protocol required |
| Confidential remote execution/TEE | **deferred, fail-closed refusal shipped** | `placement.rs:168-183`; no attestation verifier/enclave |
| Quorum/open market/DHT | **deferred** | execution memo `06`:787-792, `879-917` |
| Family e2e | **scripted CLI composition spec** | `e2e_family_team.sh`; could not execute under worker authority |
| Pilot dry-run | **shipped deterministic rehearsal** | `pilot_cmd.rs:550-1042`; smoke source inspected |
| Pilot real two-host operation/check | **partial/documented-only as turnkey** | `pilot_cmd.rs:1066-1215` |

## 4. Contradictions and drift

### `RXP-DRIFT-001` — “dispatcher wired” versus a rejecting spawn path

**`[CONTRADICTION]`** `docs/prod-audit/01-production-readiness-followup.md:114-115` marks M5 “CLOSED (implemented)” and says `RemoteRunner` plus placement wires exec into the dispatcher. Source planning does select `RemoteRunner`, but `spawn_task` returns an error and no coordinator provider driver was found (`src/dispatch/plan.rs:583-640`; `src/commands/spawn_task.rs:339-348`). Current authority: source. Resolution: open; “planner metadata wired” is accurate, “coordinator lifecycle wired” is not.

### `RXP-DRIFT-002` — Four default-on derived gates versus IC3/manual and raw-poll bypass

**`[CONTRADICTION]`** `docs/prod-audit/01-production-readiness-followup.md:103-104` says all four seams are default-on and enforcing. IC1/IC2/`msg` IC4 support that reading, but IC3 takes hand-passed trust and raw `identity poll` requires `--review` (`src/commands/identity_cmd.rs:1842-1861`; `src/cli.rs:3762-3771`). Resolution: narrow the claim to class coverage and name entry-point exceptions.

### `RXP-DRIFT-003` — “Every verdict recorded on the sigchain” versus best-effort unsigned storage

**`[CONTRADICTION]`** The decision memo requires every verdict on the same sigchain and no silent drop (`docs/content-safety-study/04-decision-memo-and-roadmap.md:310-321`, `824-840`). Live callers ignore record failures, and the record has no cryptographic signature or load-time chain validation (`src/review/verdict.rs:53-80`, `117-190`). Resolution: open. Either harden the recorder as a required signed audit boundary or rename it a best-effort local hash chain.

### `RXP-DRIFT-004` — Quorum terminology versus one ignored count

**`[CONTRADICTION]`** The content memo specifies independent reviewers and strictest-wins (`docs/content-safety-study/04-decision-memo-and-roadmap.md:422-446`, `677-696`). The deterministic path ignores `n`; the model path obtains at most weak then strong opinions and may allow strong acceptance to clear weak quarantine (`src/review/pass2_review.rs:86-99`; `src/review/reviewer.rs:277-358`). Resolution: partial/deferred, not shipped quorum.

### `RXP-DRIFT-005` — Smoke “live, not stubs” versus deterministic pilot worker

**`[CONTRADICTION]`** `tests/smoke/README.md:83-92` says smoke scenarios must hit real endpoints/binaries and stubs belong in unit tests. `pilot_dry_run` drives the real binary and real protocol, but its work product is a fixed shell command embedded in production (`src/commands/pilot_cmd.rs:43-50`, `961-979`). This is a real subprocess but deterministic fixture silicon. Resolution: describe it as protocol-live/worker-fixtured, or move the fixed-worker assertion under an explicitly permitted credential-free category.

### `RXP-DRIFT-006` — Pilot one-command/live-check claims versus real-host bootstrap

**`[CONTRADICTION]`** The runbook says `wg pilot` automates a live end-to-end check (`docs/ops/runbook.md:149-170`) and labels real per-host `up` as deploy (`docs/ops/runbook.md:176-198`). Only dry-run performs a check; real state always records no check, and no agents/listeners/remote lifecycle are started (`src/commands/pilot_cmd.rs:1066-1215`). Resolution: open, high operator impact.

### `RXP-DRIFT-007` — OpenRouter key “wired” versus existence-only check

**`[CONTRADICTION]`** `pilot.example.toml:43-47` and `docs/ops/runbook.md:163-170` say the key path is for live reviewer/workers; source prints “wired” if the file exists but neither reads it nor configures a secret/env (`src/commands/pilot_cmd.rs:1145-1159`). Resolution: open. Existence is diagnostics, not credential wiring.

### `RXP-DRIFT-008` — Older spark boundaries versus current production tree

**`[FACT]`** The decision memos correctly describe their historical wave boundaries: deterministic Pass 2, Pass-3/4 stubs, single disjoint re-run, no TEE/quorum (`docs/content-safety-study/04-decision-memo-and-roadmap.md:591-604`; `docs/execution-federation-study/06-decision-memo-and-roadmap.md:787-792`). Current source has since added conditional model review, real workers, executable verification, all four class hooks, and cross-task controls. These are historical design claims, not current inventories. Resolution: apparent conflict caused by time; preserve the memos but link a current capability matrix.

## 5. Risks and gaps

| ID | Label | Severity / likelihood | Risk or gap | Boundary and evidence |
|---|---|---|---|---|
| `RXP-RISK-001` | `[INFERENCE]` | **S1 / likely** | A daemon asked to run typed remote work reaches an unsupported `RemoteRunner` path rather than the protocol. | `RXP-006`; planner vs `spawn_task.rs:339-348` |
| `RXP-RISK-002` | `[INFERENCE]` | **S1 / possible** | Operators can mistake real `pilot up` for a running, checked family team although it only bootstraps one host. | `RXP-011`; `pilot_cmd.rs:1066-1215` |
| `RXP-RISK-003` | `[FACT]` | **S2 / possible** | Verdict audit records can be silently omitted; local records are editable without signature/link verification. | `RXP-004` |
| `RXP-RISK-004` | `[INFERENCE]` | **S2 / possible** | A result can consume its epoch before accounting/finalization, leaving a replay-blocked but incomplete graph task after failure. | `RXP-008` |
| `RXP-RISK-005` | `[FACT]` | **S2 / likely without operator loop** | Lease expiry/renewal machinery does nothing automatically unless external code invokes renew/accept/sweep. | `RXP-007` |
| `RXP-RISK-006` | `[FACT]` | **S2 / possible** | `--no-review` and raw `identity poll` can expose unscreened authenticated bytes. This is explicit policy bypass, not accidental fail-open, but needs audit/role restriction. | `cli.rs:4636-4649`; `identity_cmd.rs:1226-1239` |
| `RXP-RISK-007` | `[FACT]` | **S2 / possible** | Rejected provider results return command success through `reject(…) -> Ok(())`; automation must parse JSON rather than trust exit status. | `exec_fed_cmd.rs:1180-1193`; smoke scripts do parse JSON |
| `RXP-RISK-008` | `[UNCERTAINTY]` | **S2 / unknown** | Live ingest records do not expose deterministic/model source, and the live model eval was not rerun here. Runtime quality for the configured model remains environment-dependent. | `RXP-003`; `docs/prod-audit/02-live-reviewer-eval.md` is historical E4/E5 evidence |
| `RXP-RISK-009` | `[FACT]` | **S2 / possible** | A positive pilot TTL is accepted as “bounded,” and real startup does not install the bound into future worker processes. | `pilot_cmd.rs:213-225`, `929-941`, `1070-1215` |
| `RXP-RISK-010` | `[FACT]` | **S2 / possible** | Real peer reachability failure is only a warning; `up` still succeeds and writes state. | `pilot_cmd.rs:1219-1237` |
| `RXP-GAP-001` | `[UNCERTAINTY]` | **S3 / unknown** | Live smoke behavior could not be rerun under worker authority. | Section 7 command record |
| `RXP-GAP-002` | `[FACT]` | **S3 / likely** | No quarantine release/human queue, review flood control, real TEE, DHT, open market, or independent quorum. | Explicitly deferred in source/memos |

## 6. Recommendations

1. **`RXP-REC-001` — `[RECOMMENDATION]` (P0, execution/coordinator):** Either implement a coordinator-owned state machine that drives offer → claim transport → grant → renewal → run → accept/finalize for `RemoteRunner`, or reject `remote_provider` at task admission with an explicit “manual protocol only” status. Acceptance: a ready remote graph task completes through `wg service` in a two-home test without direct `wg provider` choreography, including restart recovery.
2. **`RXP-REC-002` — `[RECOMMENDATION]` (P0, pilot/operations):** Rename current real `pilot up` to bootstrap or narrow all operator text. Do not claim “team up” until both hosts, credentials, agent services, lease loops, default gates, a real worker, result return, and an end-to-end check are observed. Acceptance: real two-host scripted flow reports `check_passed=true`; unreachable peers/credentials fail startup when required.
3. **`RXP-REC-003` — `[RECOMMENDATION]` (P0, review/audit):** Make verdict recording part of the consumption transaction or fail closed when it cannot be persisted. Add signer identity/signature (or stop calling it a sigchain), validate every CID/link on load, and expose repair diagnostics. Acceptance: injected disk/lock/corruption failures cannot produce consumed-but-unrecorded content.
4. **`RXP-REC-004` — `[RECOMMENDATION]` (P0, result lifecycle):** Add a recoverable accept transaction spanning epoch commit, graph artifact/accounting, and terminal finalization. Acceptance: failure injection after each phase resumes idempotently and never leaves a permanently replay-blocked incomplete task.
5. **`RXP-REC-005` — `[RECOMMENDATION]` (P1, review/trust):** Publish an entry-point ingest matrix in CLI help and docs: default, bypass flag, trust source, body-withholding behavior, and audit durability for IC1–IC4. Derive IC3 cross-self trust canonically or explain why operator override is authoritative. Acceptance: every ingest command has one test for default enforcement and one explicit bypass/audit event.
6. **`RXP-REC-006` — `[RECOMMENDATION]` (P1, review):** Persist `ReviewSource`, weak/strong model ids, escalation, timeout/failure, and policy version in every verdict. Call deterministic single-pass behavior a detector, not quorum. Acceptance: `wg review log --json` distinguishes deterministic, weak, strong, and fail-closed outcomes without attacker-controlled prose.
7. **`RXP-REC-007` — `[RECOMMENDATION]` (P1, execution):** Integrate signed renewal emission/acceptance and timeout sweep into an owned service loop with observable deadlines. Acceptance: a live provider remains live; a stopped provider is automatically reclaimed; a late result is fenced, all without manual verbs.
8. **`RXP-REC-008` — `[RECOMMENDATION]` (P1, pilot/security):** Treat safe defaults as applied policy, not state labels. Install the TTL through persisted config/service environment, impose a reviewed maximum, load credentials through `wg secret` rather than a path-exists check, and report the effective peer trust map. Acceptance: `pilot status` derives each displayed control from the runtime source that enforces it.
9. **`RXP-REC-009` — `[RECOMMENDATION]` (P1, tests):** Split smoke claims into protocol-live, deterministic-fixture, credential-gated model, and real multi-host classes. Preserve the credential-free suite, but do not equate a fixed shell worker with production silicon. Acceptance: manifests and output identify which layer each pass proves.
10. **`RXP-REC-010` — `[RECOMMENDATION]` (P2, CLI):** Make security rejection machine-readable through both JSON and a documented nonzero/semantic exit contract, or provide `--fail-on-reject`. Acceptance: shell automation cannot mistake `{accepted:false}` for successful acceptance by checking exit status alone.

## 7. Evidence appendix

### 7.1 Revision and build provenance

**`[VERIFIED]`** Executed 2026-08-08 from `/home/bot/wg/.wg-worktrees/agent-8`. Exit 0:

```bash
git rev-parse HEAD
git diff --name-only b0892ea7496fd2cc8f641417a3d8e33ca9add369..HEAD
cargo build --bin wg
```

Bounded result: branch `98b319c36aa8a21fd4506fc7469fe6d58978cdda`; only `docs/audit/2026-08-08-worksgood-system/README.md` differed from the pinned production snapshot before this artifact; `cargo build --bin wg` succeeded with warnings.

### 7.2 Focused executed tests

**`[VERIFIED]`** Executed 2026-08-08; all exit 0:

```bash
cargo test --lib review::
# 53 passed; 0 failed

cargo test --lib providers::
# 54 passed; 0 failed

cargo test --lib trust::
# 7 passed; 0 failed

cargo test --bin wg pilot_cmd::tests
# 8 passed; 0 failed

cargo test --lib dispatch::plan::tests::plan_spawn_routes_remote_provider_task_to_remote_runner
# 1 passed; 0 failed
```

**`[UNCERTAINTY]`** The build/test invocations waited on shared Cargo locks and emitted unrelated warnings. They establish the named test behavior only; they do not establish remote host, live model, daemon, or operator flow behavior.

### 7.3 Attempted live smoke commands

**`[UNCERTAINTY]`** Executed in parallel on 2026-08-08 with the just-built binary first on `PATH`; all exited 1 because the audit worker lacked operator/graph authority before the scenario under test could run:

```bash
PATH="$PWD/target/debug:$PATH" bash tests/smoke/scenarios/content_safety_spark.sh
PATH="$PWD/target/debug:$PATH" bash tests/smoke/scenarios/exec_spark_borrowed_box.sh
PATH="$PWD/target/debug:$PATH" bash tests/smoke/scenarios/e2e_autowire_ingest_gate.sh
PATH="$PWD/target/debug:$PATH" bash tests/smoke/scenarios/e2e_family_team.sh
PATH="$PWD/target/debug:$PATH" bash tests/smoke/scenarios/pilot_dry_run.sh
```

Common bounded error: `worker_control.operation_refused: this command requires operator/graph authority`. No scenario result is claimed. The scripts remain E3 executable specifications inspected at:

- `tests/smoke/scenarios/content_safety_spark.sh:1-302` — standalone CLI review, deterministic credential-free path, digest/revoke assertions.
- `tests/smoke/scenarios/exec_spark_borrowed_box.sh:1-388` — real command worker, protocol choreography, fencing and disjoint verification.
- `tests/smoke/scenarios/e2e_autowire_ingest_gate.sh:1-251` — default/explicit IC4 auto-gate and derived author trust.
- `tests/smoke/scenarios/e2e_family_team.sh:1-504` — manual full composition and signed completion return.
- `tests/smoke/scenarios/pilot_dry_run.sh:1-148` — one-command dry-run, reported defaults, teardown, unsafe-config refusal.
- Manifest ownership/descriptions: `tests/smoke/manifest.toml:1937-2011`.

### 7.4 Primary source index

| Evidence | Audit use | Class |
|---|---|---|
| `src/trust.rs:1-124` | split author/provider trust, min fold | E2 |
| `src/review/mod.rs:246-461` | verdict semantics and complete pipeline | E2 |
| `src/review/reviewer.rs:277-441` | weak/strong orchestration, fail-closed, model availability | E2 |
| `src/review/pass2_review.rs:50-101` | no-scope surface and ignored deterministic quorum count | E2 |
| `src/review/verdict.rs:53-280` | hash chain, digest pin, overrides, revoke | E2 |
| `src/commands/trace_import.rs:76-134` | IC1 default-on gate | E2 |
| `src/commands/identity_cmd.rs:1090-1428` | IC4 auth-before-review and body withholding | E2 |
| `src/commands/identity_cmd.rs:1842-1997` | IC3 manual trust and real state consumption | E2 |
| `src/providers/placement.rs:142-421` | leash/filter/default refusals | E2 |
| `src/providers/lease.rs:268-500` | epoch CAS, renewal/sweep, persistence lock/refusal | E2 |
| `src/providers/verify.rs:54-145`, `155-365`, `439-536` | attribution, scope, executable/fallback re-run | E2 |
| `src/providers/worker.rs:50-199` | real command/model backend and usage | E2 |
| `src/commands/exec_fed_cmd.rs:169-1087`, `1134-1455` | CLI lifecycle, accept ordering, audit, liveness, verify | E2 |
| `src/dispatch/plan.rs:583-640`; `src/commands/spawn_task.rs:339-348` | remote planner/runtime discontinuity | E2 |
| `src/commands/pilot_cmd.rs:181-228`, `483-690`, `801-1215`, `1294-1354` | defaults, dry-run, real bootstrap, teardown | E2 |
| `pilot.example.toml:1-76`; `docs/ops/runbook.md:149-205` | operator claims/config | E4 |
| `docs/prod-audit/01-production-readiness-followup.md:100-139`, `179-205` | later readiness claims and declared residuals | E4/E5 |
| content/exec decision memos cited above | original spark/deferred boundaries | E4, historical design context |

### 7.5 Limitations

**`[UNCERTAINTY]`** This audit did not execute a live model reviewer, real cross-host network, coordinator remote task, release/installer, TEE, Telegram listener, or destructive recovery. It did not test whether an operator shell can safely supply the pilot's advertised key. Absence of a repository caller found by `rg` is strong static evidence of a manual seam but not a proof that an external deployment wrapper does not exist outside the repository.
