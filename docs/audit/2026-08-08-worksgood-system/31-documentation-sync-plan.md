# Documentation synchronization and clarification roadmap

**Audit date:** 2026-08-08

**Audit snapshot:** `b0892ea7496fd2cc8f641417a3d8e33ca9add369`

**Evidence checked through:** 2026-08-08

**Planning checkout:** `e7e58501ff13be8fccbb71ee4f1bf343bff56fea`; `git diff --name-only b0892ea7..HEAD -- . ':(exclude)docs/audit/2026-08-08-worksgood-system/**'` returned no paths, so the production and pre-existing documentation evidence cited here is byte-equivalent to the audit snapshot.

**Artifact status:** audit-only synchronization program; no proposed edit, move, archive, generator, policy, or implementation decision in this document has been applied

**Inputs:** audit charter [`README.md`](README.md), thematic syntheses [`20`](20-core-runtime-synthesis.md), [`21`](21-agency-federation-safety-synthesis.md), [`22`](22-product-docs-quality-synthesis.md), focused cutover audit [`23`](23-evaluation-evolvability-cutover.md), and contradiction register [`30`](30-contradiction-and-drift-register.md)

**Change boundary:** this new audit artifact only; production source, tests, workflows, schemas, generated output, and pre-existing documentation are untouched

## 1. Executive abstract

**`[FACT]`** WorksGood declares several bounded synchronization controls. The universal agent contract is compiled from one Markdown source, and a unit test requires the two root project guides to remain byte-identical (`src/commands/agent_guide.rs:3-15,132-185`). CI declares a Pi source-build/re-embed/diff gate (`.github/workflows/ci.yml:174-201`). Those controls were inspected, not executed by this task. In contrast, the pre-artifact planning checkout counted 619 files below `docs/`, including 570 Markdown files, but had no `docs/manifest.toml`, `docs/product-contract.toml`, machine-readable decision index, or machine-readable glossary (command in §7.2). `docs/KEY_DOCS.md:1-5` still calls itself the canonical key-doc list and is dated 2026-04-29.

**`[INFERENCE]` (high confidence)** No repository-wide documentation authority system was located. This is falsified by identifying an applicable estate/claim/decision/term registry or equivalent enforcement surface omitted from §2.1's search.

**`[FACT]`** A parser declaration and dispatch currently disagree: `src/cli.rs:527-557` advertises five `wg done` flags that `src/main.rs:1261-1274` rejects. Test policy and dispatch also disagree: the smoke contract says owned scenarios gate Done (`tests/smoke/README.md:1-29`), while the current Done dispatch contains no smoke invocation (`src/main.rs:1261-1274`; `src/commands/completion_done.rs:29-294`, independently checked in audit 30 `P1/P2`).

**`[INFERENCE]` (high confidence)** The estate therefore does not have one safe global linear authority order. Accepted decisions govern desired policy; reachable implementation and executed evidence govern current behavior in their scope. Public support needs a **join record** that preserves disagreement, not another prose source selected by age.

**`[INFERENCE]` (high confidence)** Documentation drift is the visible symptom of incomplete authority migrations. Completion, onboarding/model routing, evaluation, federation/review/remote execution, and historical evidence each have old and new representations that remain simultaneously discoverable. Rewriting all prose before settling those authorities would create a newer-looking but equally ungrounded layer.

**`[RECOMMENDATION]`** Run the program in six ordered phases:

1. freeze unsupported claims and publish a baseline/decision ledger;
2. make bounded factual corrections where current behavior is already known;
3. adjudicate product semantics and code-behavior choices that prose cannot decide;
4. establish machine-readable estate, claim, glossary, generation, and evidence contracts;
5. synchronize current guides and migrate the information architecture without deleting history;
6. turn the contracts into release, CI, review, and scheduled drift controls.

The phases intentionally separate **F** factual synchronization, **D** human decision, **I** implementation/behavior, **S** structural documentation, and **V** verification work. A factual edit may describe or narrow current behavior; it may not silently choose desired behavior. A design decision may not be presented as shipped until its implementation and executable acceptance evidence land.

**`[RECOMMENDATION]` Priority:** immediately correct or visibly qualify false high-impact operator/security claims (`WGDR-003`, `WGDR-008`–`WGDR-012`, `WGDR-017`, `WGDR-029`–`WGDR-042`, `WGDR-049`) while routing contested semantics (`WGDR-001`, `WGDR-002`, `WGDR-005`, `WGDR-006`, `WGDR-013`–`WGDR-016`, `WGDR-021`, `WGDR-028`, `WGDR-035`, `WGDR-040`, `WGDR-042`, and `WGDR-T01`–`WGDR-T12`) through the decision queue in §4. No bulk move, archive, or historical rewrite should precede the manifests and owner decisions.

**`[FACT]` Requirement-to-plan map:** this table is navigation, not evidence that the recommendations are implemented.

| Required roadmap element | Where this artifact supplies it |
|---|---|
| target authority hierarchy | §2.2 policy/behavior/support/evidence join |
| canonical indexes and glossary | §2.3 estate manifest, decision applicability index, product-contract, glossary and evidence index |
| target document architecture | §2.4 |
| what to update, decide, implement, generate, test and archive | §§3.3–3.9, with F/D/I/S/V types |
| migration phases and dependency order | §§3.3–3.9 and §6.1 |
| contradiction traceability | WGDR ranges in every backlog table; full program acceptance in §5.3 |
| quick corrections versus structural work | §§3.4 and 3.6–3.8 |
| code behavior versus human adjudication | §§3.5 and 4.1–4.2 |
| owners/domains | §2.5 |
| automated drift-prevention checks | §3.9 |
| acceptance and rollback/archive policy | §§5.2–5.3 |
| uncertainty and resolved safeguards | §4.3 |

### 1.1 One-page execution control board

**`[RECOMMENDATION]`** Use this board to operate the program; the later sections provide evidence, exact trace allocations, and rollback detail. Nothing in the board is implemented by this audit.

| Control plane | Canonical target | First accountable action | Release acceptance |
|---|---|---|---|
| Authority | scope/question first; accepted human decision for intended policy; executed exact-candidate behavior for current behavior; product contract joins them without hiding disagreement | Product council names approvers and opens `DEC-01`–`DEC-12` | no public/safety claim is `current` without applicable decision (where normative), reachable source, selected evidence, owner and docs |
| Estate/navigation | `docs/manifest.toml` owns completeness; generated routers own audience selection | Documentation steward classifies every tracked doc/root Markdown path or explicit ignore | orphan/move/delete/duplicate-ID fixtures fail; compatibility paths and owners resolve |
| Decisions | `docs/decision-index.toml` owns section-scoped acceptance/applicability and human receipts | Product/security/domain approvers classify proposed, accepted, rejected, superseded and decision-required material | no generator, test or document-status edit can self-ratify policy |
| Claims/support | `docs/product-contract.toml` joins scoped claim, decision, implementation, evidence, audience/support and rollback | Domain owners register high-impact `WGDR` claims first | parser-only, unselected, skipped, broken and deferred surfaces cannot render as supported |
| Vocabulary | `docs/glossary.toml` owns qualified terms, aliases and “not the same as” relations | Owners adjudicate `WGDR-T01`–`WGDR-T12` through `DEC-11` | cross-plane reference is generated/linted; historical text is exempted, not rewritten |
| Evidence | generated result index distinguishes pass/fail/skip/inspected/not-selected by exact revision/artifact/environment | Test/release owner captures `P0-04` baseline | required lanes cannot pass with zero assertions; real journey/security negatives are selected |
| Current guidance | four journeys, conceptual spine, generated reference and rollback-aware runbooks | Factual packages `F-ENTRY/LIFE/MODEL/AGENCY/SEC/EVIDENCE` correct dangerous claims without choosing policy | release-binary human flows and scoped limitation matrices agree with the contract |
| History/archive | manifest retention plus immutable bundle applicability/supersession indexes | Evidence/docs owners index bundles before any physical move | body hashes unchanged; append-only errata; stable lookup/replacement or explicit none |

**`[RECOMMENDATION]` Release train and dependency fence:**

1. **Contain (Phase 0):** freeze the register, name owners, label dangerous claims `broken/partial/decision-required`, and record honest evidence classes.
2. **Correct (Phase 1):** land domain-scoped factual edits only; no moves, broad rewrites, historical body edits, or disguised behavior choices.
3. **Decide and implement (Phase 2):** humans resolve product/security/lifecycle semantics; code/help/tests/docs then move together, while unresolved rows remain visible.
4. **Build contracts (Phase 3):** establish estate, decision, claim, glossary, generator, link and evidence schemas; warning-only import precedes blocking enforcement.
5. **Migrate (Phases 4–5):** rewrite supported journeys/reference/runbooks, add destinations and compatibility stubs, then index/archive immutable evidence.
6. **Prevent recurrence (Phase 6):** after one clean warning-only release, block unclassified docs/surfaces/decisions/tests/generated deltas and stale public claims.

**`[RECOMMENDATION]` Non-negotiable close and rollback fence:** every `WGDR-001`–`WGDR-049`, `WGDR-T01`–`WGDR-T12`, `WGDR-R01`–`WGDR-R12`, and `WGDR-U01`–`WGDR-U12` gets an owner, disposition, evidence class and successor/decision link. **F** copy may close while **D/I** remains open; a decision may close while implementation remains deferred. Current public/safety changes land with their contract/evidence disposition. Moves retain compatibility for at least one release by default; generator rollback restores tool plus outputs; historical corrections are sidecar errata; uncertainty never becomes pass by prose.

## 2. Scope, authority map, and target document architecture

### 2.1 Scope and method

**`[FACT]`** This plan read every required input in full, the documentation leaf's target-architecture section, prior sync artifacts (`docs/design/doc-sync-system.md`, `docs/doc-sync-audit-2026-04-29.md`, `docs/audit/doc-sync-apr12-delta-checklist.md`), and direct primary surfaces: root/docs landing pages, `KEY_DOCS`, manual source declarations, the sync script, command parser/dispatch, the bundled agent-guide test, CI, and smoke policy/manifest. It did not use `AGENTS.md` as sole product evidence.

**`[VERIFIED]`** This plan executed repository-shape and byte-equivalence commands only. It did not successfully execute a source-built product flow: `cargo run --quiet --bin wg -- done --help` exceeded a 300-second compile-lock budget and is not pass evidence.

**`[FACT]`** Audit 30 independently completed the candidate-built help command and records its bounded result in `30-contradiction-and-drift-register.md:13-22,35-55,259-273`.

**`[FACT]`** No `.github/CODEOWNERS` file was found by the exact command in §7.2.

**`[UNCERTAINTY]`** This roadmap does not know the human maintainers who will own each domain.

**`[RECOMMENDATION]`** Owner names below are accountable **roles/domains**; Phase 0 must map each to a named person or team before dispatch.

### 2.2 Target authority model: a two-axis join, not “newest file wins”

**`[RECOMMENDATION]`** Adopt the following authority rules.

| Question | Primary authority | Required corroboration | What may be generated | Prohibited shortcut |
|---|---|---|---|---|
| What should the product promise? | accepted, scoped product decision or ADR | implementation owner, migration status, negative constraints | capability/maturity and decision tables | treating shipped code or a passing test as ratification |
| What does this revision do? | reachable source/dispatch/schema at an identified revision | executed behavior for the claimed environment; otherwise label inspected-only | CLI/reference/current-behavior tables | treating parser help, comments, filenames, or AGENTS alone as behavior |
| What is publicly supported? | one `product-contract` claim joining policy, source, audience and support level | positive/negative behavior test and release selection | public CLI, journey and platform matrices | equating “discoverable” with unattended/public/supported |
| What does a term mean? | approved glossary entry linked to concrete types/commands | schema/enum/source references and namespace | glossary and diagrams | collapsing agency agent, runtime worker, federated principal, or model/compute provider |
| What passed? | exact CI/release receipt for exact commit/artifact | selected target/scenario class, result and skips | evidence dashboard | treating a test/scenario file as executed assurance |
| What happened historically? | immutable report at its observed revision | external applicability/supersession record | evidence bundle index | editing old conclusions into present tense or archiving by age alone |

**`[RECOMMENDATION]` Explicit precedence hierarchy within a scoped question:**

1. **Scope first:** exact revision/artifact, audience, command surface, deployment and time bound every claim. Evidence from another scope cannot outrank evidence in the claimed scope.
2. **Normative intent:** an accepted, applicable product decision/ADR outranks proposed designs, manuals, reports and comments for what the product *should* promise. If no accepted decision exists, authority is `unknown`; implementation does not self-ratify policy.
3. **Current behavior:** executed behavior of the exact candidate (E1) outranks source inference for that environment/input; reachable implementation/schema/build configuration (E2) outranks unexecuted tests and prose. A failing current flow is still evidence of behavior, not desired policy.
4. **Public support join:** `docs/product-contract.toml` is the canonical public-status record only when it cites the applicable decision, reachable dispatch, selected executable evidence and owner. It records disagreement; it does not override missing enforcement.
5. **Derived reference:** generated CLI/schema/config/compatibility tables outrank hand-copied reference for the fields they generate, and identify their source revision/generator.
6. **Authored current guidance:** journeys, explanations and runbooks interpret the contract and generated reference for an audience; they may add rationale but not contradict or silently widen them.
7. **Historical/contextual evidence:** reports, studies, designs and archives remain authoritative only for what they observed or proposed at their pinned revision. External supersession/applicability indexes route readers to current status without rewriting history.

**`[INFERENCE]`** Source wins only the “what current bytes encode” question. An ADR wins only the “what accepted policy intends” question. When they disagree, the contract status is `drift`, `partial`, `broken`, or `decision-required`; neither side is silently rewritten to match the other. The hierarchy above therefore orders evidence **after scope and question are fixed**; it is not a global “newest file wins” ladder.

### 2.3 Canonical registries and generated views

**`[RECOMMENDATION]`** Create five distinct canonical records. Their separation prevents the old `KEY_DOCS` failure mode, where a curated reading list impersonated a complete estate inventory, and prevents a document-status field from silently ratifying product policy.

1. **`docs/manifest.toml` — estate inventory.** One record per tracked documentation artifact (or explicit ignore): stable doc ID, path, kind, audience, authority class, owner, `valid_as_of_revision`, source, generated outputs, `supersedes`, `superseded_by`, claim IDs, decision IDs, evidence IDs, retention class, and redirect/alias paths. Path is mutable metadata; doc ID is stable. Its status describes the artifact, not acceptance of the decisions it contains.
2. **`docs/decision-index.toml` — decision applicability.** One record per proposed or accepted product decision: stable decision ID, scope, state (`proposed|accepted|superseded|rejected|decision-required`), approving authority and receipt, effective/superseding revision, affected claim IDs, migration/rollback, and source sections. Section-level records are required where one ADR mixes accepted and deferred material. Only the named human authority may change acceptance state; a generator or test cannot self-ratify it.
3. **`docs/product-contract.toml` — claims and supported journeys.** One record per public or safety-relevant claim: stable claim ID, scoped statement, status (`current|partial|broken|deferred|historical|decision-required`), support level (`public|advanced|internal|migration|hidden`), terms, applicable decision IDs, source/dispatch sites, behavior tests, CI/release lane, docs, owner, last verified revision/result, bypasses, and supersession.
4. **`docs/glossary.toml` — namespaced vocabulary.** Stable term ID, preferred display, namespace, definition, source types/commands, allowed aliases, deprecated aliases, “not the same as” links, and decision/status. It must cover `WGDR-T01`–`WGDR-T12` before broad rewrites.
5. **Evidence/result index — generated from CI and immutable bundles.** It records `executed-pass`, `executed-fail`, `skip`, `inspected-not-run`, `not-selected`, commit/artifact, environment, and evidence class. It must not be hand-edited into a pass.

**`[RECOMMENDATION]`** Generate or check these views from the registries and primary schemas:

- `docs/README.md`: curated audience router generated from manifest queries;
- `docs/KEY_DOCS.md`: compatibility path generated as a **curated view**, with its completeness claim removed;
- `docs/reference/cli.md` and compatibility `docs/COMMANDS.md`: public signatures from public contract + reachable dispatch, with authored examples in keyed include blocks;
- `docs/reference/config.md`, lifecycle/status, storage/schema, package/platform, compatibility constants, trust/authority, evaluation-plane, ingest-seam, capability/maturity, and supported-journey tables;
- bundle indexes for ADRs/designs, audits/reports/incidents, research/studies/plans, and archives;
- website/manual derivatives with source revision and generator version.

**`[FACT]`** Source and test/workflow definitions already encode this pattern for two bounded surfaces: `AGENT_GUIDE_TEXT` uses `include_str!` and declares parity tests (`src/commands/agent_guide.rs:3-15,132-185`), while CI declares a Pi re-embed-and-diff step (`.github/workflows/ci.yml:174-201`). This task did not execute either control.

**`[RECOMMENDATION]`** Generalize those declared controls rather than inventing a timestamp-based “freshness” detector.

### 2.4 Target information architecture

**`[RECOMMENDATION]`** Treat this as a target map, not an instruction to move files immediately.

```text
README.md                                  # product promise + supported journeys
AGENTS.md == CLAUDE.md                     # tool-required project layer; parity tested

docs/
  README.md                                # curated router generated from manifest
  manifest.toml                            # complete estate inventory
  decision-index.toml                      # accepted/proposed/superseded decision applicability
  product-contract.toml                    # claims, journeys, support, evidence joins
  glossary.toml                            # canonical namespaced vocabulary

  getting-started/
    attended-existing.md
    new-graph-only.md
    unattended-automation.md
    install-and-upgrade.md

  concepts/
    README.md
    system-model.typ                       # one declared conceptual source graph
    generated/                             # md/pdf/diagrams/tables

  reference/
    cli.md
    config.md
    lifecycle-and-status.md
    storage-and-schemas.md
    compatibility-and-packaging.md
    authority-and-trust.md
    capability-maturity.md

  operations/
    README.md
    runbooks/{day-2,recovery,release,federation}.md
    troubleshooting/
    security-and-secrets.md
    platform-support.md

  architecture/
    README.md                               # accepted/proposed/superseded index
    adr/
    designs/

  contributor/
    development.md
    testing.md
    documentation.md
    worktrees-and-publication.md

  evidence/
    README.md
    audits/<date-or-release>/
    reports/<topic>/
    incidents/
    test-results/

  research/
    README.md
    studies/
    plans/

  archive/
    README.md                               # retention, provenance, redirects
    <year>/<topic>/
```

**`[RECOMMENDATION]`** Preserve existing paths as generated compatibility files, stubs, or aliases until inbound links and external compatibility are reviewed. `docs/design/` and `docs/designs/` must not be merged until every file has status and successor metadata (`WGDR-043`, `WGDR-047`; `DOC-REC-010`).

### 2.5 Owner/domain model

| Role/domain | Accountable scope | Required approval |
|---|---|---|
| Product council/maintainer | product sentence, supported journeys, public/internal surface, cycle/retry/model-plane policy | every `D-*` decision and public claim status |
| Documentation steward | manifest schema, IA, style, generated views, redirects, archive policy | factual/structural doc changes; cannot decide product semantics |
| Domain owner: lifecycle/completion | task states, admission, Done, smoke, cycles, legacy completion | `WGDR-001`–`WGDR-007`; `WGDR-U01`, `WGDR-U02`, `WGDR-U04` |
| Domain owner: model/config/operations/release | launcher, routes, profiles, config, doctor, accounting, packaging/platform | `WGDR-008`–`WGDR-018`; `WGDR-U05`, `WGDR-U07`, `WGDR-U10` |
| Domain owner: agency/evaluation/chat/functions | persona/assignment/evaluation/learning/history/human flows | `WGDR-019`–`WGDR-027`; `WGDR-U08`, `WGDR-U11` |
| Security council + Fed/Review/Exec/Pilot owners | custody, recovery, transport, trust/review, remote lifecycle, pilot | `WGDR-028`–`WGDR-042`; `WGDR-U09`, `WGDR-U12` |
| Test/release infrastructure | evidence classes, CI selection, smoke ownership, release receipts | `WGDR-002`, `WGDR-044`–`WGDR-046`, every executable acceptance check |
| Web/manual/tooling owner | source DAGs, converters, generated files, link/asset checks | `WGDR-043`, `WGDR-U06` |

**`[RECOMMENDATION]`** Every backlog item remains unassignable until the role is resolved to a named owner and reviewer. Security-sensitive claims require a security reviewer distinct from the prose author; generated public commands require the CLI domain owner and test owner.

## 3. Findings and phased synchronization backlog

### 3.1 Program findings

#### `SYNC-001` — the immediate task is claim containment, not mass rewriting

- **Label/state:** `[INFERENCE]`; current planning conclusion.
- **Severity/likelihood/confidence:** S1 where false setup/security/release claims authorize human action; observed text conflict and likely human-action risk; high.
- **Affected boundary/owner:** public setup, safety, release and operations claims; product council plus documentation steward and relevant domain owner.
- **Evidence:** 49 open drift records include current operator/security conflicts; current source and primary docs directly disagree on launcher behavior, completion flags, smoke, and manual source authority (`WGDR-001`–`WGDR-003`, `WGDR-008`, `WGDR-029`–`WGDR-042`, `WGDR-043`).
- **Recommendation:** factual corrections and visible limitations may proceed; disputed intended behavior must enter §4.

#### `SYNC-002` — a complete inventory and a curated router are different products

- **Label/state:** `[FACT]` + `[INFERENCE]`; current gap.
- **Severity/likelihood/confidence:** S2; observed inventory ambiguity; high.
- **Affected boundary/owner:** all documentation discovery; documentation steward/tooling.
- **Evidence:** `KEY_DOCS` calls itself canonical and is dated 2026-04-29 (`docs/KEY_DOCS.md:1-5`); the pre-artifact planning checkout had 619 docs files and no estate manifest. Audit 16 measured substantial post-index growth (`16-documentation-information-architecture.md:238-253,587-612`).
- **Recommendation:** manifest owns completeness; routers own audience selection.

#### `SYNC-003` — command reference must join parser, dispatch, support policy, and behavior

- **Label/state:** `[FACT]`; current.
- **Severity/likelihood/confidence:** S1 for Done/smoke, otherwise S2; observed; high.
- **Affected boundary/owner:** CLI support and release evidence; CLI, completion and test-infrastructure owners.
- **Evidence:** Done flags exist in Clap but are rejected in dispatch (`src/cli.rs:527-557`; `src/main.rs:1261-1274`). CI selects only one named integration target in its integration job (`.github/workflows/ci.yml:126-162`) despite 176 top-level targets in the planning checkout.
- **Recommendation:** generate public reference from contract records whose tests are selected, not from Clap alone.

#### `SYNC-004` — current documentation needs namespaced vocabulary and maturity, not one “unified identity/trust” slogan

- **Label/state:** `[INFERENCE]`; partial.
- **Severity/likelihood/confidence:** S2; likely cross-plane misreading; high.
- **Affected boundary/owner:** identity, agency, model, trust, review and remote-execution narratives; product/domain owners.
- **Evidence:** the typed authority synthesis distinguishes persona, process, principal, route, local trust assertion, capability, review and completion (`21-agency-federation-safety-synthesis.md:15-48,79-184`); register terms `WGDR-T01`–`WGDR-T12` preserve collisions.
- **Recommendation:** approve glossary entries and generate namespace/maturity tables before global replacements.

#### `SYNC-005` — historical immutability and current discoverability are compatible

- **Label/state:** `[FACT]` + `[INFERENCE]`; positive pattern/gap.
- **Severity/likelihood/confidence:** S2 if readers reverse a closed security conclusion; likely without navigation; high.
- **Affected boundary/owner:** audit/report consumers and evidence governance; documentation steward plus evidence owner.
- **Evidence:** a point-in-time federation audit remains correct for its revision while current source implements a later handshake (`WGDR-R09`); old sync reports currently label many point-in-time reports simply “Current” (`docs/KEY_DOCS.md:82-99,360-376`).
- **Recommendation:** preserve report bytes; add external applicability/supersession indexes.

#### `SYNC-006` — prior “auto doc-sync task” design is useful only as a secondary control

- **Label/state:** `[DOC-CLAIM]` + `[RECOMMENDATION]`; proposed design narrowed.
- **Severity/likelihood/confidence:** S2 process risk; likely if used as the only gate; high.
- **Affected boundary/owner:** feature delivery and release admission; documentation tooling plus release owners.
- **Evidence:** `docs/design/doc-sync-system.md:1-15,82-224` proposes a Markdown feature manifest and post-completion AI task. It predates publication-derived completion and assumes staleness is primarily missing coverage. Current evidence shows parser/dispatch/test/decision conflicts that a diff-reading agent cannot adjudicate.
- **Recommendation:** same-change contract classification is the release control. An asynchronous doc task may catch explanatory omissions, but cannot waive claim registration, generated parity, executable evidence, or human decisions.

#### `SYNC-007` — generation requires a declared DAG and fail-closed conversion

- **Label/state:** `[FACT]`; current gap.
- **Severity/likelihood/confidence:** S2; observed source ambiguity and possible malformed derivative; high.
- **Affected boundary/owner:** manual and website outputs; web/manual/tooling owner.
- **Evidence:** the manual README calls unified `wg-manual.typ` authoritative while retaining chapters as working originals (`docs/manual/README.md:30-42`); the sync script calls Typst source of truth, converts chapters, concatenates Markdown, and on converter failure copies raw Typst into `.md` (`scripts/sync-docs.sh:1-8,66-118`).
- **Recommendation:** decide source nodes; remove “copy raw as Markdown” from release generation; regenerate-and-diff in CI.

#### `SYNC-008` — drift prevention must fail on unclassified deltas, not on file age

- **Label/state:** `[INFERENCE]`; recommended control.
- **Severity/likelihood/confidence:** S2; likely recurrent drift; high.
- **Affected boundary/owner:** every public/schema/test/package delta; documentation tooling, domain and release owners.
- **Evidence:** accepted historical files may be old, while undated designs may already be superseded (`WGDR-047`, `WGDR-048`, `WGDR-R08`, `WGDR-R09`). Existing Pi and guide parity gates detect derivative drift by exact source relationship rather than timestamps.
- **Recommendation:** gate new public surfaces, enum variants, docs, tests, binaries, constants, and generated outputs against manifests; use age only to request review, never to infer falsehood.

### 3.2 Work types and change rules

| Type | May do | Must not do | Minimum acceptance |
|---|---|---|---|
| **F — factual sync** | describe verified/reachable current behavior, qualify scope, fix broken path/example, add known limitation | select desired semantics, claim unrun behavior, rewrite historical observation | cited authority, owner review, relevant link/help/journey check |
| **D — human decision** | choose product policy, support level, threat model, term, retention | masquerade as implementation | decision record, alternatives, migration/rollback, named approver |
| **I — implementation/behavior** | make code/tests/help match an accepted decision | update docs alone and call behavior fixed | failing test first where applicable, human flow/security negative, CI/release selection |
| **S — structural docs** | add manifests, generators, indexes, redirects, reorganize after mapping | bulk-move/delete before inventory/links/owners | schema tests, clean regeneration, redirect/link/archive checks |
| **V — verification** | execute and record exact evidence class | turn skip/not-selected/compile into pass | exact artifact/revision/environment/result and durable receipt |

#### 3.2.1 Traceability allocation and close rule

**`[RECOMMENDATION]`** Use this as the program-level allocation check. The detailed tables below remain authoritative for dependencies and acceptance; this matrix prevents a range from disappearing between the quick-correction, decision, structural, and verification queues.

| Register allocation | Factual/current-state package | Human decision or implementation gate | Structural/verification successor |
|---|---|---|---|
| `WGDR-001`–`WGDR-007` | `F-LIFE` | `DEC-01`, `DEC-02`, `D-LIFE` | `S-CLI`, `S-SCHEMA`, lifecycle human flows |
| `WGDR-008`–`WGDR-018` | `F-ENTRY`, `F-MODEL`, `F-EVIDENCE` | `DEC-03`, `DEC-04`, `D-PRODUCT`, bounded `I-CONTROL-INTEGRITY` | `S-CONTRACT`, `S-CLI`, journey/platform evidence |
| `WGDR-019`–`WGDR-027` | `F-AGENCY` | `DEC-05`, `DEC-06`, `D-AGENCY` | `S-SCHEMA`, evaluation/learning evidence |
| `WGDR-028`–`WGDR-039` | `F-SEC` | `DEC-07`, `DEC-08`, `D-SECURITY` | `S-DECISIONS`, `S-SCHEMA`, adversarial evidence |
| `WGDR-040`–`WGDR-042` | `F-SEC` | `DEC-09`, `D-REMOTE` | remote/Pilot failure and restart evidence |
| `WGDR-043`–`WGDR-048` | `F-EVIDENCE` | `DEC-04`, `DEC-10`, retention owner decisions | `S-MANIFEST`, `S-DAG`, `S-LINKS`, `S-EVIDENCE`, `A-*` |
| `WGDR-049` | current limitation in `P0-02` | bounded `I-CONTROL-INTEGRITY` repair | typed IPC replay/idempotency evidence |
| `WGDR-T01`–`WGDR-T12` | qualified terms in domain factual packages | `DEC-11`, `D-TERMS` | `S-GLOSSARY` and terminology lint |
| `WGDR-R01`–`WGDR-R12` | preserve as non-issue/resolved guard | no reopen without new contrary evidence | regression guard linked from contract/evidence index |
| `WGDR-U01`–`WGDR-U12` | retain `unknown`/`suspected-drift` | only the bounded check in §4.3 may change status | `V-UNCERTAINTIES` with pass/fail/skip/unknown receipt |

**`[RECOMMENDATION]` Close rule:** a row closes only when its machine-readable ledger entry names (1) the exact claim/status change, (2) decision state if normative, (3) implementation state if behavioral, (4) executed or explicitly unexecuted evidence class, (5) current documentation/compatibility paths, and (6) owner approval. A factual prose correction can close its **F** work item while the contradiction remains open as `decision-required` or `broken`; a decision can close its **D** item while implementation remains `deferred`. This prevents “docs synchronized” from erasing unresolved product or code work.

### 3.3 Phase 0 — baseline, containment, and ownership (P0; before mass edits)

| Item / type | Trace | Deliverable and dependency | Acceptance / rollback |
|---|---|---|---|
| `P0-01` **S/V** freeze baseline | all `WGDR-001`–`WGDR-049`, `WGDR-T01`–`WGDR-T12`, `WGDR-R01`–`WGDR-R12`, `WGDR-U01`–`WGDR-U12` | Export register rows into a machine-readable temporary ledger; assign named owner/reviewer and disposition. No dependency. | Every ID maps to exactly one backlog item, decision, accepted debt, resolved guard, or uncertainty test. Rollback: delete generated temporary output; audit files remain immutable. |
| `P0-02` **F** claim containment notices | `WGDR-001`–`WGDR-003`, `WGDR-008`–`WGDR-012`, `WGDR-021`, `WGDR-022`, `WGDR-029`–`WGDR-042`, `WGDR-049` | Add narrowly scoped current-behavior/known-limitation notices only where a false claim can cause immediate action; depends on owner assignment. | Notice cites source and says `broken/partial/decision-required`; does not promise future fix. Revert atomically if source check is wrong. |
| `P0-03` **D** authority council | `WGDR-005`, `WGDR-006`, `WGDR-013`–`WGDR-016`, `WGDR-028`, `WGDR-035`, `WGDR-T01`–`WGDR-T12`, `WGDR-U01`, `WGDR-U02`, `WGDR-U12` | Convene product, security, lifecycle, docs and test owners; open decision records listed in §4. | Each record has deadline, approver, alternatives and no prose “resolution” before acceptance. |
| `P0-04` **V** evidence baseline | `WGDR-002`, `WGDR-044`–`WGDR-046`, `WGDR-U04`, `WGDR-U05`, `WGDR-U07`, `WGDR-U09`, `WGDR-U10` | Capture checkout-built help, selected CI/test inventory, links/assets, generators, and current journey results in explicit evidence classes. | Fail/skip/not-selected preserved. A timed-out compile is not pass. Baseline artifact is content-addressed or pinned to commit. |

### 3.4 Phase 1 — bounded factual corrections (P0/P1; no moves)

These packages may run in parallel after `P0-01`; packages touching the same source-of-truth or generated derivative remain sequential.

| Item / type / owner | Contradiction trace | Update scope | Acceptance |
|---|---|---|---|
| `F-ENTRY` **F**, product docs + launcher | `WGDR-008`, `WGDR-009`, `WGDR-018`, `WGDR-043`, `WGDR-R01`, `WGDR-U06` | Split attended existing graph, new route-free graph, unattended automation, and upgrade paths across root/docs landing/install/quickstart. Qualify website equivalence until generated. | Checkout-built release-binary human flows list mutations, credentials, plugin/profile/service effects and rollback; no existing-graph Pi prerequisite claim survives. |
| `F-LIFE` **F**, lifecycle docs | `WGDR-001`–`WGDR-007`, `WGDR-T08`, `WGDR-T09`, `WGDR-U01`, `WGDR-U04` | Publish current lifecycle/completion reachability matrix; correct status/dependency/wait/current v3 path; label legacy/special paths and unsupported flags. Do not decide smoke/cycle/retry policy. | Matrix names parser, dispatch, durable evidence and tests; source paths compile; contested rows say decision-required. |
| `F-MODEL` **F**, model/config/ops | `WGDR-010`–`WGDR-018`, `WGDR-T03`, `WGDR-T04`, `WGDR-U05`, `WGDR-U07`, `WGDR-U10` | Correct preservation/unknown-key limitations, accounting scope, handler-vs-surface matrix, Pi worker/RPC topology, fallback and package/upgrade facts. | Each example resolves to an allowed surface; route-aware checks pass or limitation is explicit; package differences remain decision-required. |
| `F-AGENCY` **F**, agency/evaluation/functions/chat | `WGDR-019`–`WGDR-027`, `WGDR-T01`, `WGDR-T02`, `WGDR-T06`, `WGDR-T08`, `WGDR-T10`, `WGDR-U08`, `WGDR-U11`, `WGDR-R10` | Separate completion review receipt, candidate evaluation record and agency performance evaluation; document manual/dormant assignment, current learning disconnect, function schema/planner and onboarding transaction limits. | Authority/effect table says whether each plane gates lifecycle, records cost, feeds learning or schedules work; no synthetic task restoration is implied. |
| `F-SEC` **F**, security/Fed/Review/Exec/Pilot | `WGDR-028`–`WGDR-042`, `WGDR-T03`, `WGDR-T07`, `WGDR-T12`, `WGDR-U09`, `WGDR-U12`, `WGDR-R06`–`WGDR-R09` | Narrow “accepted/complete/custodied/ACL/sigchain/quorum/all seams/dispatcher wired/turnkey” to exact current enforcement and deferred boundary. | Security reviewer confirms every claim names enforcement site and negative gap; historical studies remain unchanged; known S1 gaps are conspicuous. |
| `F-EVIDENCE` **F**, docs/test/release | `WGDR-043`–`WGDR-048`, `WGDR-R03`, `WGDR-U06`, `WGDR-U07`, `WGDR-U10` | Remove completeness/currentness claims from curated indexes, classify smoke evidence, update supersession/status metadata externally, correct compile-only diagnosis. | No “complete/current/passed” assertion lacks scope and evidence class; `KEY_DOCS` is labeled curated until generated. |

**`[RECOMMENDATION]` Initial exact-path review list:** these are candidates to inspect/update, not a license to edit every path. `docs/manifest.toml` ultimately owns the complete mapping.

| Package | Pre-manifest paths to review together |
|---|---|
| `F-ENTRY` | `README.md`; `docs/README.md`; `docs/guides/install.md`; `docs/quickstart-pi-openrouter.md`; `website/quickstart-pi-openrouter.html`; launcher help source `src/bin/worksgood.rs` (generate/verify, do not hand-copy) |
| `F-LIFE` | `docs/manual/02-task-graph.{typ,md}`; `docs/manual/04-coordination.{typ,md}`; `docs/COMMANDS.md`; `docs/AGENT-GUIDE.md`; `src/text/agent_guide.md`; `tests/smoke/README.md`; current CLI/dispatch sources as evidence |
| `F-MODEL` | `docs/models.md`; `docs/config-precedence.md`; `docs/config-ux-design.md`; `docs/AGENT-SERVICE.md`; profile/setup/doctor sections of `README.md`, `docs/README.md`, and generated reference |
| `F-AGENCY` | `docs/AGENCY.md`; `docs/manual/03-agency.{typ,md}`; `docs/manual/05-evolution.{typ,md}`; `docs/design-pi-evaluation-plane.md`; `docs/design-worker-owned-universal-review.md`; evaluation/assignment/function reference sections |
| `F-SEC` | `docs/ADR-fed-000-acceptance-brief.md` and `ADR-fed-001..004`; content/exec acceptance briefs and ADRs; federation/content/exec studies; `docs/ops/runbook.md`; Pilot/federation/review/compute-provider reference sections |
| `F-EVIDENCE` | `docs/KEY_DOCS.md`; `docs/COMMANDS.md`; `docs/manual/README.md`; `scripts/sync-docs.sh`; `tests/smoke/README.md`; `.github/workflows/ci.yml`; report/study/design bundle indexes to be created |

**`[RECOMMENDATION]`** Quick correction packages should be small, domain-scoped commits. They may link to a known issue rather than duplicate volatile implementation detail. They must not perform path moves, archive bodies, generated-derivative hand edits, or broad search-and-replace.

### 3.5 Phase 2 — adjudicate policy and behavior (P0/P1; decision queue in §4)

| Item / type | Trace | Dependency / outcome | Acceptance |
|---|---|---|---|
| `D-LIFE` **D→I/F** | `WGDR-001`, `WGDR-002`, `WGDR-005`, `WGDR-006`, `WGDR-007`, `WGDR-U01`, `WGDR-U02`, `WGDR-U04` | Decide Done flags, owned smoke, cycles, Abandoned retry and legacy reachability; then implement/test or remove/narrow. | One state/command table across parser, operator/worker dispatch, manual, agent contract and CI; real human/worker flows. |
| `D-PRODUCT` **D→I/F** | `WGDR-008`, `WGDR-009`, `WGDR-013`–`WGDR-018`, `WGDR-043`, `WGDR-U05`, `WGDR-U06`, `WGDR-U07`, `WGDR-U10` | Decide Pi scope, handler support, package/platform/public command membership, source/website relationship. | Scoped product sentence and support matrix; release archive membership and route-aware readiness tests. |
| `D-AGENCY` **D→I/F** | `WGDR-019`–`WGDR-023`, `WGDR-T01`, `WGDR-T02`, `WGDR-T06`–`WGDR-T08`, `WGDR-U08` | Decide identity mutability, auto-assignment product, review ledger/virtual projections, learning semantics and credit. | Preserve v3 as sole lifecycle consumer unless an explicit contrary ADR passes; exactly-once/non-authoritative acceptance from audit 23. |
| `D-SECURITY` **D→I/F** | `WGDR-028`–`WGDR-039`, `WGDR-T07`, `WGDR-T12`, `WGDR-U09`, `WGDR-U12` | Ratify or mark experimental; choose custody/recovery/history/revocation/message sealing/review quorum/bypass policies. | Threat-model decisions, adversarial tests, and docs status agree; no test alone ratifies ADR. |
| `D-REMOTE` **D→I/F** | `WGDR-040`–`WGDR-042` | Choose coordinator-owned remote lifecycle vs explicit manual-only; choose Pilot bootstrap vs turnkey. | Restart/failure-injection two-home flow if shipped; otherwise admission/help/runbook reject turnkey interpretation. |
| `D-TERMS` **D/S** | `WGDR-T01`–`WGDR-T12`, `WGDR-R04`, `WGDR-R05`, `WGDR-R10`, `WGDR-R12` | Ratify namespaced glossary and maturity vocabulary after domain decisions. | Each ambiguous term has preferred qualified forms, aliases and source types; generated lint checks cross-plane public text. |
| `I-CONTROL-INTEGRITY` **I→F/V** | `WGDR-010`, `WGDR-011`, `WGDR-012`, `WGDR-049`; bounded factual defects from `WGDR-004`, `WGDR-026` | Repair behaviors where an existing contract is already unambiguous (stateful IPC response delivery, lossless/validated config edits within the approved key policy, dated/scope-correct accounting, admission/onboarding atomicity). Product choices such as extension namespaces or override roles remain decision-gated. | Regression fails on the snapshot and passes after the fix; real socket/config/dated-accounting/admission flow runs; factual docs and contract status update in the same candidate. |
| `V-UNCERTAINTIES` **V** | `WGDR-U01`–`WGDR-U12` | Execute the bounded call-graph, fault, provider, generator, toolchain, platform and adversarial checks in §4.3; can proceed per domain after baseline. | Result remains pass/fail/skip/unknown with exact revision/environment; no inconclusive check closes its row. |

### 3.6 Phase 3 — structural contracts and generators (P1; after schema and relevant decisions)

| Item / type | Trace | Deliverable / dependency | Acceptance |
|---|---|---|---|
| `S-MANIFEST` **S** | `WGDR-043`, `WGDR-047`, `WGDR-048`, all bundle/navigation drift | Implement `docs/manifest.toml` schema and importer; depends on `P0-01`. | 100% of tracked docs/root Markdown classified or explicitly ignored; unique IDs/paths; referenced owners/files/claim/decision IDs exist. |
| `S-DECISIONS` **S/D** | `WGDR-005`, `WGDR-006`, `WGDR-013`–`WGDR-016`, `WGDR-028`, `WGDR-035`, `WGDR-040`, `WGDR-042`, all `DEC-01`–`DEC-12` | Implement `docs/decision-index.toml` and import accepted/proposed/superseded status without changing it; depends on named approving authorities and status vocabulary. | Every normative claim cites an applicable decision or says `decision-required`; acceptance changes require a human receipt; mixed ADRs use section-level applicability. |
| `S-CONTRACT` **S/V** | `WGDR-001`–`WGDR-049` | Implement `docs/product-contract.toml`; depends on decision status vocabulary, not necessarily all decisions resolving. | Every public/safety claim and supported journey has applicable decision/status, source, test selection, owner and docs; unresolved rows are machine-visible. |
| `S-GLOSSARY` **S** | `WGDR-T01`–`WGDR-T12` | Implement glossary and source mappings; depends on `D-TERMS`. | Enum/schema additions and forbidden unqualified cross-plane terms create actionable failures, with allowlisted historical exemptions. |
| `S-CLI` **S/V** | `WGDR-001`, `WGDR-014`, `WGDR-018`, `WGDR-043`, `DOC-003`, `DOC-005` | Generate public CLI from support contract + reachable dispatch; keyed authored examples; depends on `D-LIFE/D-PRODUCT`. | Parser-only flag cannot appear as supported; every public command has positive/negative release-binary test; internal commands are tagged/hidden. |
| `S-SCHEMA` **S/V** | `WGDR-003`, `WGDR-019`–`WGDR-023`, `WGDR-033`, `WGDR-037`–`WGDR-039`, terminology rows | Generate lifecycle/status, identity/trust, review/evaluation, ingress and maturity tables from reviewed schemas/contracts. | Adding/changing variant requires disposition; round-trip/schema tests and clean regeneration pass. |
| `S-DAG` **S/V** | `WGDR-043`, `WGDR-U06`, `SYNC-007` | Declare manual/website/organizational-pattern source DAG; fail closed on converter failure. | Clean checkout regeneration is diff-free; output embeds source revision/generator; raw Typst cannot be published as `.md` on failure. |
| `S-LINKS` **S/V** | `WGDR-043`, `DOC-RISK-004` | Policy-aware current-doc link/anchor/asset checker with archive exemptions. | Current/public broken local links/assets fail; exemption has owner/reason/expiry; historical absolute evidence is classified rather than blindly rewritten. |
| `S-EVIDENCE` **S/V** | `WGDR-002`, `WGDR-044`–`WGDR-046`, all claims | Generate selected-target/scenario/release evidence dashboard and orphan/delta checks. | Required lanes cannot pass with zero assertions; skip/not-selected visible; all 176 integration targets and 324 smoke entries classified at baseline. |

### 3.7 Phase 4 — synchronize current docs and migrate IA (P1/P2)

| Item / type | Dependency | Work and acceptance |
|---|---|---|
| `M-JOURNEYS` **F/S/V** | `F-ENTRY`, `D-PRODUCT`, `S-CONTRACT` | Rewrite four supported journeys; release-binary human flows pass on declared platforms; root README becomes promise/router, not full reference. |
| `M-CONCEPTS` **F/S** | `F-LIFE/F-AGENCY/F-SEC`, `D-TERMS`, `S-SCHEMA` | Rebuild conceptual order: task → dependency → generation/attempt → evidence/completion, then service/agency/model/federation overlays. Generated glossary/diagrams match schemas. |
| `M-REFERENCE` **S/V** | `S-CLI/S-SCHEMA/S-DAG` | Replace hand-maintained command/config/status/compat tables with generated outputs while retaining keyed examples. Clean regeneration and link checks pass. |
| `M-OPS` **F/S/V** | behavior decisions and route-aware implementation | Consolidate day-2, recovery, release, federation, secrets/platform runbooks. Each step names effect, evidence, failure, rollback and last-tested receipt. |
| `M-PATHS` **S** | `S-MANIFEST`, all destination owners | Publish path-by-path mapping and add destinations first. Use Git-aware moves only after inbound-link/external-path review; compatibility stubs/aliases remain for at least one release by default. |
| `M-ROOT` **S** | `M-PATHS` | Apply approved root allowlist. Every non-allowlisted root file has destination/retention/disposition; tool-required files and tested agent-guide parity are preserved. |

### 3.8 Phase 5 — evidence, archive, and supersession migration (P1/P2)

| Item / type | Trace | Acceptance |
|---|---|---|
| `A-BUNDLES` **S** | `WGDR-028`, `WGDR-036`, `WGDR-043`, `WGDR-047`, `WGDR-048`, `WGDR-R08`, `WGDR-R09` | Every audit/report/study/design/incident bundle has an index with observed revision, current applicability, closure and successor; historical body hashes remain unchanged. |
| `A-STATUS` **S/F** | `WGDR-028`, `WGDR-047`, `WGDR-048` | Accepted/proposed/implemented/partial/superseded metadata is section-scoped where needed; product decision owner approves, tests never self-ratify policy. |
| `A-DESIGNS` **S** | `WGDR-043`, `WGDR-047` | Inventory all `design/` and `designs/` files before consolidation; duplicates need provenance and owner decision; no age-only merge/delete. |
| `A-RETENTION` **D/S** | all historical/accepted-debt rows | Ratify retention classes and external-link compatibility window. Archive only with replacement or explicit “no current replacement,” reason and stable lookup. |

### 3.9 Phase 6 — continuous drift prevention (release gate)

| Check | Trigger | Failure condition | Rollback/escalation |
|---|---|---|---|
| Manifest delta | every PR | new/moved/deleted doc unclassified; duplicate stable ID/path; invalid supersession | block; revert manifest/output together |
| Decision-index delta | ADR/decision state or applicability change | acceptance changed without named human approval/receipt; supersession cycle; affected claims undisposed | block; restore prior state or record a new explicit decision |
| Product-contract delta | public command/flag, enum, config key, binary, compat constant, safety enforcement or journey change | no claim disposition/owner/decision/evidence lane | block or explicitly mark internal/deferred with approval |
| Regenerate-and-diff | every PR touching source DAG | generated output differs or generator/source revision absent | block; regenerate with pinned tool or revert source |
| Parser-dispatch-support join | CLI changes | public parser item unreachable/rejected without contract status; dispatch path undocumented | block; add behavior test and generated reference |
| Evidence selection | tests/CI/smoke/release changes | target/scenario exists but is not selected/classified; required class has zero assertions; skip budget exceeded | block required lane; advisory lane reports loudly |
| Link/asset/redirect | current/public docs and moves | missing target/anchor/asset, expired exemption, removed compatibility path | block; restore path or approved redirect/stub |
| Historical integrity | evidence/archive changes | immutable body hash changes without correction protocol | block; use sidecar/index; correction requires append-only erratum |
| Owner/freshness review | scheduled and on source change | current public/safety claim lacks owner or reviewed revision after affected source changed | mark stale/decision-required and block affected release claim |
| Human-flow canary | supported journey change/release | actual TUI/terminal/browser/install flow differs from docs | block the journey's support badge; do not substitute library test |
| Quarterly scatter-gather audit | schedule | contradiction/uncertainty has no owner/disposition or new orphan surfaces | create bounded tasks; quarterly audit supplements, never replaces PR gates |

**`[RECOMMENDATION]`** A PR template should require: affected claim IDs, documentation impact, generated outputs, behavior/evidence class, archive/redirect impact, decision dependency, and rollback. “No docs needed” must identify the contract row proving the change is internal. An asynchronous doc-sync task may follow only for non-release-blocking explanatory improvement; it must not permit current public/safety drift to merge.

## 4. Contradictions, uncertainty, and human decision queue

### 4.1 Decision queue

These are not factual copy edits. The indicated human authority must decide them before final wording or structural retirement.

| Decision ID / priority | Human question | Register trace | Recommended default, not current fact | Required decision artifact and acceptance |
|---|---|---|---|---|
| `DEC-01` P0 completion | Are legacy Done flags removed or restored? Does owned smoke gate Done, publication, or neither? | `WGDR-001`, `WGDR-002`, `WGDR-007`, `WGDR-U01` | Keep v3 publication-derived completion sole authority; bind any required smoke to immutable publication evidence, not legacy flags. | Accepted lifecycle decision; operator+worker parser/dispatch/help/tests/agent contract one table. |
| `DEC-02` P0 lifecycle | Are cycles supported under v3? Is Abandoned reversible? Is manual claim an explicit override? | `WGDR-004`, `WGDR-005`, `WGDR-006`, `WGDR-U02`, `WGDR-R12` | Default claim shares admission; override/restore is explicit, reasoned, fenced. | Lifecycle ADR and human-flow fixtures for pause/time/cycle/retry. |
| `DEC-03` P0 product/model | Is Pi sole attended, sole recommended, or sole overall plane? Which handlers are unattended? | `WGDR-009`, `WGDR-014`, `WGDR-015`, `WGDR-016`, `WGDR-U05` | Scope “sole” to attended/recommended unless strict worker admission and tests say otherwise. | Product sentence + surface capability matrix + route-aware doctor/setup tests. |
| `DEC-04` P1 public surface | Which commands, binaries (Casa), install modes and platforms are supported? | `WGDR-013`, `WGDR-018`, `WGDR-043`, `WGDR-044`, `WGDR-U07`, `WGDR-U10` | Explicit public/advanced/internal/source-only/platform states; no discovery-based implication. | Product/release manifest, archive tests and platform evidence. |
| `DEC-05` P0 evaluation/agency | How are review attempts represented and how does accepted work feed learning? What is performance? | `WGDR-019`–`WGDR-023`, `WGDR-T01`, `WGDR-T02`, `WGDR-T06`, `WGDR-T08`, `WGDR-U08` | Adopt audit 23: append-only review attempt ledger + virtual non-schedulable projection + separate exactly-once learning projector; v3 alone controls lifecycle. | Evaluation ADR including credit, anti-gaming, migration and 12 acceptance tests from audit 23 §6.4. |
| `DEC-06` P0 identity/human | What binds agency persona, `wgid`, runtime worker and human classification? | `WGDR-019`, `WGDR-T01`, `WGDR-T02`, `WGDR-T07`, `WGDR-T08`, synthesis `XAUTH-005` | Signed/authorized binding record; never derive local trust from self-asserted metadata or an unaudited `--human` boolean. | Product/security record with rotation/evolution and mistaken-human negative tests. |
| `DEC-07` P0 federation governance | Are Fed ADRs accepted, or is implementation experimental? What custody/recovery/history/revocation policy is promised? | `WGDR-028`–`WGDR-036`, `WGDR-U12`, `WGDR-R06`, `WGDR-R07` | Do not claim hostile-worker custody until separate authenticated signer exists; preserve accepted no-offline-FS debt explicitly. | Accepted threat model, custody/recovery ADR updates and adversarial acceptance. |
| `DEC-08` P0 review | Is audit best-effort or required? Is Pass 2 escalation or independent quorum? Which bypasses are allowed? | `WGDR-037`–`WGDR-039`, `WGDR-U09` | Required durable digest-bound record at enforcing edges; call current model path escalation until quorum exists; audit high-value bypass. | Review policy matrix, signed/tamper-verified persistence decision, live source provenance tests. |
| `DEC-09` P0 remote/Pilot | Is remote placement coordinator-owned or manual? Is Pilot bootstrap or turnkey? | `WGDR-040`–`WGDR-042` | Reject automatic/turnkey interpretation until owned restart-safe state machine and real-host check exist. | WG-Exec/Pilot decision plus two-home restart/failure flow or explicit admission/help refusal. |
| `DEC-10` P1 docs source graph | Is unified Typst or chapter Typst canonical? Is website generated here or an external consumer? | `WGDR-043`, `WGDR-U06` | One declared DAG; generated site/manual outputs only; converter failure is fatal. | Docs build ADR, pinned generator, clean regen and website digest/link test. |
| `DEC-11` P1 vocabulary | Ratify namespaced meanings and aliases for all collision terms. | `WGDR-T01`–`WGDR-T12`, `WGDR-R04`, `WGDR-R05`, `WGDR-R10` | Use qualified nouns at cross-plane boundaries; keep compatibility spellings as explicit aliases only. | Approved glossary with type links and generated public tables. |
| `DEC-12` P1 persistence claim | What crash/platform guarantee is public? | `WGDR-U03`, `WGDR-U11` | Promise only tested process-crash/Unix bounds until parent fsync and cross-platform locking/fault tests exist. | Persistence/platform ADR and fault/concurrency evidence. |

### 4.2 Factual edits versus product/code decisions

| Example | Factual synchronization allowed now | Decision/implementation required |
|---|---|---|
| Bare `worksgood` | Say existing graph opens setup-neutral TUI and new graph route-free bootstrap (`src/bin/worksgood.rs:6-16,124-151`). | Decide marketing scope of Pi; change launcher only after `DEC-03`. |
| Done/smoke | Say current dispatch rejects flags and v3 Done consumes completion evidence. | `DEC-01` chooses whether smoke returns and where; code/test/help then change together. |
| Config preservation | Remove “preserves comments” claim for current setter. | A lossless edit is implementation work under the existing preservation contract; unknown-key/extension policy still needs an explicit product choice. |
| Worker message IPC | Say current array-valued stateful reads can mutate state before response serialization fails (`WGDR-049`). | Non-flattened typed response plus replay/idempotency tests are implementation work, not a documentation decision. |
| Spend/metrics | Say current spend is invocation-date grouped and metrics are process-local (`WGDR-012`). | Correct completion timestamps/persisted scope are implementation work; any intentionally process-local product surface must be renamed by decision. |
| Evaluation | Say universal completion review does not currently feed agency learning and reviewer usage is discarded. | `DEC-05` chooses ledger, learning semantics, identity/credit and migration. |
| Federation custody | Say current custodian loads same-user secrets in-process. | `DEC-07` decides supported deployment boundary and separate signer architecture. |
| Review quorum | Say current deterministic count is ignored and model path escalates weak→strong. | `DEC-08` chooses required independent quorum and audit durability. |
| Remote/Pilot | Say planner metadata exists, spawn rejects remote, and real Pilot path is bootstrap. | `DEC-09` chooses/manual vs owned lifecycle and “turnkey” acceptance. |
| Old reports | Add external supersession/applicability link. | Never rewrite old evidence to agree with current code; corrections use append-only errata. |

### 4.3 Uncertainty disposition

**`[RECOMMENDATION]`** `WGDR-U01`–`WGDR-U12` remain open until their proposed checks execute. They must appear in the contract as `unknown` or `suspected-drift`, never be converted to `current` by this roadmap. Specifically:

- call-graph/path fixtures: `WGDR-U01`, `WGDR-U08`;
- lifecycle/platform/concurrency/fault tests: `WGDR-U03`, `WGDR-U04`, `WGDR-U11`;
- live provider/reviewer/child argv evidence: `WGDR-U05`, `WGDR-U09`;
- generator/website/toolchain provenance: `WGDR-U06`, `WGDR-U07`;
- exact-archive Windows/macOS runtime: `WGDR-U10`;
- security policy/adversarial chain tests: `WGDR-U12`.

**`[RECOMMENDATION]`** Resolved/non-issue rows `WGDR-R01`–`WGDR-R12` become regression guards, not cleanup targets. Preserve route-free init, attended discovery, agent-guide parity, fail-closed trust resolution, separate assigned/agency fields, accepted offline-FS limitation, task-scoped Exec grants, immutable historical reports, distinct completion/performance evaluation, worker capability interception, and the narrowed cycle uncertainty.

## 5. Risks, safeguards, and rollback/archive policy

### 5.1 Program risks

| ID | Severity / likelihood | Risk | Safeguard |
|---|---|---|---|
| `SYNC-RISK-001` | S1 / likely | Factual rewrite silently chooses a disputed product policy. | Type every item F/D/I/S/V; contract status `decision-required`; domain approver required. |
| `SYNC-RISK-002` | S1 / possible | Docs are made internally consistent with unsafe overclaims rather than enforcement. | Security claims require enforcement site + negative test + separate security review. |
| `SYNC-RISK-003` | S1 / observed pattern | Parser/help/test inventory is mistaken for reachable/release behavior. | Join parser + dispatch + support decision + selected behavior receipt. |
| `SYNC-RISK-004` | S2 / likely | New manifests become another stale index. | Orphan/delta gates on docs, commands, enums, tests, binaries, constants and outputs; generated views only. |
| `SYNC-RISK-005` | S2 / likely | Bulk moves break external links, agent discovery or history. | Add destinations first, map inbound links, keep compatibility paths, move by Git, one domain per change. |
| `SYNC-RISK-006` | S2 / possible | Archiving by age hides still-current evidence; editing reports destroys provenance. | Archive by applicability, immutable body hashes, external supersession/errata records. |
| `SYNC-RISK-007` | S2 / possible | Generators erase authored nuance or publish malformed fallback output. | Keyed authored blocks, pinned generators, fail-closed conversion, semantic owner review. |
| `SYNC-RISK-008` | S2 / likely | “No docs needed” and asynchronous doc tasks allow release drift. | Same-change claim disposition is mandatory; async task is secondary and cannot waive gate. |
| `SYNC-RISK-009` | S2 / possible | Tests make claims stronger than environment exercised. | Evidence classes and exact receipts; skip/not-selected/fixture/static labels visible. |
| `SYNC-RISK-010` | S2 / possible | Compatibility stubs become permanent duplicate authorities. | Stubs generated from manifest, contain replacement/status only, owner and expiry/review release. |

### 5.2 Rollback policy by change class

1. **Factual corrections:** one domain and authority per commit/PR. Revert the entire correction if its cited source/behavior is disproved. Do not revert by restoring an unqualified false claim; replace with `uncertain` and link the new evidence.
2. **Decisions and behavior:** decision, implementation, tests, contract status and generated docs land atomically where feasible. If implementation rolls back, the contract returns to `partial/deferred/broken` in the same rollback and the superseded decision remains in history.
3. **Generators:** pin generator/tool versions and store the source graph. A generator upgrade is isolated from content changes where possible. Rollback restores generator plus all outputs; partial generated-output rollback is forbidden.
4. **Path moves:** destination and compatibility path land before removal. Default compatibility window is at least one release and two successful link scans; external/public paths may require indefinite stubs. Rollback restores old paths from the same content ID without deleting the new destination prematurely.
5. **Archives:** archive is a metadata/path operation, not deletion. Preserve original commit/revision/date/content digest and provide replacement or “no current replacement.” Rollback moves the same bytes back and updates indexes; no history rewriting.
6. **Historical evidence:** body is append-only/immutable by policy. Corrections are sidecar errata or bundle-index entries with author/date/reason. A mistaken erratum is superseded by another record, not erased.
7. **Deletions:** allowed only after owner approval, manifest disposition, zero required inbound links, retention/legal/security review, replacement or explicit none, and a recovery commit. Git presence alone is not the user-facing rollback plan.

### 5.3 Acceptance criteria for the whole program

The synchronization program is complete only when all of the following hold:

- every `WGDR-001`–`WGDR-049`, `WGDR-T01`–`WGDR-T12`, `WGDR-R01`–`WGDR-R12`, and `WGDR-U01`–`WGDR-U12` has a machine-readable disposition, owner, evidence and successor/decision link;
- all tracked docs/root Markdown are in the estate manifest or reviewed ignore list; every current/public/safety document has owner and valid revision;
- every accepted/proposed/superseded decision has section-scoped applicability, named approval evidence, affected claims and migration/rollback in the decision index;
- every public/safety claim joins accepted policy (where normative), reachable source, behavior evidence class, CI/release selection and generated docs;
- current command reference has no parser-only supported flag and no unclassified public command;
- glossary tables distinguish the authority namespaces and maturity states in `WGDR-T01`–`WGDR-T12`;
- the four supported operator journeys pass actual release-binary human-flow tests on declared platforms and list effects/rollback;
- current lifecycle, evaluation, trust/review, ingest, remote and maturity matrices agree with accepted decisions and source;
- manual, website, command, schema and compatibility regeneration is deterministic and diff-free from a clean checkout;
- required CI/evidence classes cannot pass with zero assertions; skips and unselected tests are visible;
- current links/assets/anchors pass; moves retain approved compatibility paths;
- historical evidence body hashes remain unchanged and every bundle routes to current applicability/closure;
- a deliberate unclassified command, enum, doc, binary, test, compat constant and stale generated output each fails the appropriate gate in a negative fixture;
- `git diff --check`, manifest/schema tests, generators, link checks, domain tests and declared human/security flows pass on the exact candidate revision.

## 6. Recommendations and dependency ordering

### 6.1 Critical path

```text
P0 owner/baseline/containment
  ├── factual packages F-ENTRY/F-LIFE/F-MODEL/F-AGENCY/F-SEC/F-EVIDENCE
  └── decision queue DEC-01..12
          │
          ├── accepted behavior changes + executable evidence
          └── S-MANIFEST + S-DECISIONS + S-CONTRACT + S-GLOSSARY
                    │
                    ├── S-CLI/S-SCHEMA/S-DAG/S-LINKS/S-EVIDENCE
                    │         │
                    │         └── M-JOURNEYS/M-CONCEPTS/M-REFERENCE/M-OPS
                    │                    │
                    │                    └── M-PATHS/M-ROOT
                    └── A-BUNDLES/A-STATUS/A-DESIGNS/A-RETENTION
                                          │
                                          └── Phase-6 hard release gates
```

**`[RECOMMENDATION]`** `S-MANIFEST` can begin before every product decision because it can record `decision-required`. Generated public behavior tables cannot become authoritative until their decisions and behavior checks resolve. IA moves wait for manifest, links, owners and redirects. Archive work may index immutable bundles early, but physical consolidation waits for status inventory.

### 6.2 Release increments

1. **Increment A — safety truthfulness:** claim containment, first-journey correction, current lifecycle/evaluation/security limitation matrices. No moves.
2. **Increment B — governance substrate:** schemas, estate/claim/glossary registries, baseline import, warning-only orphan reports.
3. **Increment C — adjudicated current contract:** accepted decisions and implementations; generated reference/schema/maturity views; required behavior tests selected.
4. **Increment D — navigation and IA:** curated router, current guides/runbooks, path compatibility stubs, bundle indexes.
5. **Increment E — hard gates:** after one warning-only release with zero unexplained findings, make manifest, generation, links, evidence selection and public-surface delta checks blocking.

**`[RECOMMENDATION]`** Do not couple all 49 contradictions into one mega-change. Domain packages can land independently when their contract rows and dependencies are explicit. Security/authority corrections should not wait for cosmetic IA. Conversely, no domain may declare completion merely because its prose was updated while its accepted implementation decision remains open.

### 6.3 Completion handoff for downstream synthesis

**`[RECOMMENDATION]`** The comprehensive audit synthesis should carry forward these conclusions without presenting them as implemented:

- target authority is a policy/behavior join recorded in a product contract, with human decision applicability kept in a separate decision index;
- estate manifest, decision index, claim registry and namespaced glossary have separate duties;
- v3 candidate/publication completion is the current ordinary-task authority pending human decision, while special-path reachability remains incompletely inventoried; review visibility/learning is a proposed ledger/projector architecture, not shipped;
- quick factual correction, structural docs work and product/code decisions are separate queues;
- historical evidence remains immutable and gains applicability/supersession routing;
- drift prevention relies on declared derivatives and unclassified-delta gates, not timestamps or AI follow-up alone.

## 7. Evidence appendix

### 7.1 Direct primary evidence inspected

| Evidence | Direct observation | Class |
|---|---|---|
| `README.md:87-165` | unqualified bare-launch Pi/plugin and “sole model plane” claims | E4 `[DOC-CLAIM]` |
| `src/bin/worksgood.rs:6-16,124-151` | existing/new bare launcher is setup-neutral/route-free; advanced options use concierge | E2 `[FACT]` |
| `docs/README.md:1-84,145-200` | eight-state/legacy validation and Claude-default first-time setup narrative | E4 `[DOC-CLAIM]` |
| `src/cli.rs:527-557`; `src/main.rs:1261-1274` | Done flags parsed then rejected; bare route is `completion_done::run` | E2 `[FACT]` |
| `tests/smoke/README.md:1-29`; `tests/smoke/manifest.toml:1-17` | documented Done gate, owner/exit contract, grow-only claim | E4/E3 `[DOC-CLAIM]` / inspected spec |
| `.github/workflows/ci.yml:68-201` | selected library/formal/one integration/Pi embed lanes; Pi regenerate-and-diff control | E2 `[FACT]` |
| `src/commands/agent_guide.rs:3-15,132-185` | embedded single source and root-guide byte-parity test | E2/E3 `[FACT]` / inspected test |
| `docs/KEY_DOCS.md:1-16,360-376` | canonical/complete/current language and 2026-04-29 date | E4 `[DOC-CLAIM]`, dated-aging |
| `docs/manual/README.md:30-42`; `scripts/sync-docs.sh:1-8,66-118` | conflicting source declarations and raw-Typst fallback copied to Markdown | E2/E4 `[FACT]` / `[DOC-CLAIM]` |
| `docs/design/doc-sync-system.md:1-15,82-224` | earlier proposed feature manifest + post-completion AI task design | E4 `[DOC-CLAIM]`, proposed/historical context |
| `docs/doc-sync-audit-2026-04-29.md:1-30,185-237,273-292` | prior fan-out sync, many explicit deferrals, quarterly pattern | E5 historical context |

### 7.2 Commands executed for this plan

**`[VERIFIED]`** Static inventory/provenance command, cwd `/home/bot/wg/.wg-worktrees/agent-21`, checkout `e7e58501`, 2026-08-08, exit 0:

```bash
git rev-parse HEAD
find docs -type f | wc -l
find docs -type f -name '*.md' | wc -l
find . -maxdepth 1 -type f -name '*.md' | wc -l
test -e docs/manifest.toml
test -e docs/product-contract.toml
test -e docs/glossary.toml
cmp -s AGENTS.md CLAUDE.md
find .github -maxdepth 2 -type f -iname '*owner*' -o -name CODEOWNERS
rg -c '^\[\[scenario\]\]' tests/smoke/manifest.toml
find tests -maxdepth 1 -type f -name '*.rs' | wc -l
git diff --name-only b0892ea7496fd2cc8f641417a3d8e33ca9add369..HEAD \
  -- . ':(exclude)docs/audit/2026-08-08-worksgood-system/**'
```

Bounded result:

```text
revision=e7e58501ff13be8fccbb71ee4f1bf343bff56fea
pre-artifact docs files=619; docs Markdown=570; root Markdown=56
manifest/product-contract/glossary absent
AGENTS.md == CLAUDE.md; no CODEOWNERS-like file found below `.github/`
smoke entries=324; top-level integration targets=176
production/pre-existing-doc delta from audit snapshot: none
```

**`[VERIFIED]`** The retry audit checked the proposed decision-index path against the pinned planning tree without changing that tree:

```bash
if git cat-file -e e7e58501ff13be8fccbb71ee4f1bf343bff56fea:docs/decision-index.toml \
  2>/dev/null; then echo present; exit 1; else echo absent; fi
```

Result on 2026-08-08: `absent`, exit 0. This verifies path absence only; it does not prove that no prose decision records exist.

**`[VERIFIED]`** Metadata/source-declaration command, same environment, exit 0:

```bash
rg -l '^(\*\*)?(Status|Last updated|Date|Valid as of|Applies to)(\*\*)?:' \
  docs --glob '*.md' | wc -l
rg -n '^Last updated:' docs/KEY_DOCS.md
rg -n 'source of truth|authoritative|working originals' \
  docs/manual/README.md scripts/sync-docs.sh
```

Bounded result: 281 of the 570 pre-artifact docs Markdown files matched this deliberately narrow status-like regex; `KEY_DOCS` reports 2026-04-29; the manual README and sync script make the conflicting declarations quoted in §3 `SYNC-007`. Nonstandard metadata may be missed, so 281 is not a complete status audit.

**`[UNCERTAINTY]`** `cargo run --quiet --bin wg -- done --help | head -40` timed out after 300 seconds under compile contention and is not behavior evidence. Audit 30's candidate-built command is the inherited E1 record for rendered help; this plan independently inspected parser and dispatch source.

### 7.3 Input traceability

- Charter structure/evidence/decision separation: [`README.md`](README.md), especially §§3–6.
- Core lifecycle/model authority and recommendations: [`20-core-runtime-synthesis.md`](20-core-runtime-synthesis.md), especially `CORE-001..012`, `CORE-DRIFT-001..012`, and §6.
- Typed authority/trust and security seams: [`21-agency-federation-safety-synthesis.md`](21-agency-federation-safety-synthesis.md), especially `XAUTH-001..010` and §6.
- Target product/IA/contract/evidence model: [`22-product-docs-quality-synthesis.md`](22-product-docs-quality-synthesis.md), especially §§2.4–2.6 and `PRODUCT-REC-001..015`.
- Evaluation representation and learning decisions: [`23-evaluation-evolvability-cutover.md`](23-evaluation-evolvability-cutover.md), especially §§5–6.
- Deduplicated contradiction authority: [`30-contradiction-and-drift-register.md`](30-contradiction-and-drift-register.md), `WGDR-001`–`WGDR-049`, `WGDR-T01`–`WGDR-T12`, `WGDR-R01`–`WGDR-R12`, `WGDR-U01`–`WGDR-U12`.

### 7.4 Limitations

- **`[FACT]`** This task modified no production source, test, workflow, schema, package, generated derivative, or pre-existing documentation; the exact task output relative to its integrated `main` is this new artifact.
- **`[FACT]`** No full Cargo/smoke suite, generator, link checker, browser/TUI flow, release archive, external provider, federation, review, remote execution, Pilot, installer, Windows or macOS runtime was executed here.
- **`[UNCERTAINTY]`** Proposed file names, schema fields, IA paths, compatibility windows and owner roles require implementation/owner review. They are recommendations, not hidden current systems.
- **`[UNCERTAINTY]`** Documentation counts include the growing audit bundle and are orientation data, not semantic coverage.
- **`[UNCERTAINTY]`** This program cannot certify correctness or security. It is designed to prevent unsupported claims and make residual uncertainty and deferred behavior difficult to hide.
