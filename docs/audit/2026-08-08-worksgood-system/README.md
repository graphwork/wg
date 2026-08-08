# WorksGood repository-wide system audit

**Audit date:** 2026-08-08

**Audit snapshot:** `b0892ea7496fd2cc8f641417a3d8e33ca9add369` (commit time 2026-08-07T12:38:38+02:00)

**Artifact status:** charter and entry point; thematic findings are not yet complete

**Scope root:** the repository at the snapshot above, including tracked source, tests, documentation, packaging, and operator surfaces

**Change boundary:** audit artifacts under this directory only; no production source, tests, or pre-existing documentation may be changed by this audit

**`[CHARTER-RULE]`** This README is the normative contract for every artifact in
this audit. It also conforms to the seven-part structure that it requires of the
rest of the tree. A path or test name below is evidence of repository structure,
not by itself a claim that the represented behavior is correct or has been
executed.

## 1. Executive abstract

**`[INFERENCE]`** The breadth and overlap of the repository make any single
narrative an unsafe proxy for the implemented system.

**`[FACT]`** At the pinned snapshot, the Rust library exposes graph, service,
agency, identity, provider,
review, trust, streaming, and worker-control modules (`src/lib.rs:20-117`);
Cargo declares four binaries, including the separate Casa adapter
(`Cargo.toml:23-41`); and the repository contains distinct Rust, Pi/TypeScript,
formal, installer, documentation, and smoke-test surfaces. The initial inventory
counted 434 Rust files below `src/`, 176 top-level Rust test targets, 324 smoke
scenarios, and 603 files below `docs/` (exact command and limitations in section
7).

**`[CHARTER-RULE]`** The audit uses a **fractal evidence contract**: a reader may stop at
the final executive synthesis, descend into a thematic synthesis, or continue
to a leaf audit without losing the local abstract, map, findings, drift, risk,
recommendation, and evidence trail. Parallel leaf audits fan in through three
explicit syntheses; contradictions are preserved in a separate register; a
skeptical review occurs before the final synthesis.

**`[CHARTER-RULE]`** The charter does **not** assume that a documented claim is
implemented, that an implemented branch is reachable, or that a test file has
run. Every empirical or evaluative statement must be labeled as an observed
fact, verified behavior, documented claim, inference, recommendation,
contradiction, or uncertainty. Normative audit instructions are labeled
`[CHARTER-RULE]`. Contradictory claims remain visible until evidence and
authority resolve them.

## 2. Scope and system map

### 2.1 In scope

**`[CHARTER-RULE]`** The audit covers the repository-wide relationships listed
below. Unless explicitly qualified, each path and structure observation is a
**`[FACT]`** at the pinned snapshot:

1. **Product and executable boundaries:** the `wg`, `worksgood`, `nex`, and
   `casa-adapter` binaries declared in `Cargo.toml:23-41`; the library export
   surface in `src/lib.rs:20-144`; and CLI parsing/dispatch beginning at
   `src/main.rs:17-48`, `src/cli.rs:11-38`, and `src/commands/mod.rs:1-198`.
2. **Graph and persistence:** task/status/work-graph representations
   (`src/graph.rs:382-382`, `src/graph.rs:689-689`, and
   `src/graph.rs:2705-2705`); JSONL load/save/modify and locking
   (`src/parser.rs:285-395`); atomic runtime files (`src/atomic_file.rs:1-127`);
   completion and recovery reducers, including the save-transaction phases
   (`src/save_transaction.rs:1-139`); and `.wg`/`.workgraph` resolution
   (`src/workgraph_dir.rs:1-68`).
3. **Orchestration and completion:** task commands, service/coordinator,
   dispatch, claim/attempt runtime, worktree isolation, cycles/waits/cron,
   evaluation, finalization, completion evidence, and recovery paths under
   `src/commands/`, `src/service/`, `src/dispatch/`, `src/evaluation/`, and the
   completion-related modules exported by `src/lib.rs:30-45`.
4. **Model and interaction planes:** config/profile/setup, handler routing,
   native/Nex, Pi, Claude, Codex and OpenCode adapters, stream translation,
   accounting, chat/concierge, TUI, HTML/server, Telegram/Matrix, and notification
   surfaces under `src/config*.rs`, `src/profile/`, `src/executor/`,
   `src/pi_*`, `src/stream_event.rs`, `src/chat*.rs`, `src/tui/`, `src/html.rs`,
   and `src/notify/`.
5. **Agency and federated trust/safety:** `src/agency/`, `src/identity/`,
   `src/federation.rs`, `src/providers/`, `src/review/`, `src/trust.rs`, the
   corresponding commands, schemas, ADRs/studies, and their executable evidence.
6. **Verification:** inline/unit tests, 176 top-level Rust test files,
   fixtures/snapshots, `tests/install/`, `tests/upgrade/`, the 324-entry smoke
   manifest and scenario directory, the formal Lean surface, Pi package tests,
   CI, and release-contract checks. The smoke gate's stated owner/exit-code
   contract is in `tests/smoke/README.md:7-29`; its manifest declares itself
   grow-only at `tests/smoke/manifest.toml:1-17`.
7. **Documentation and concepts:** root documentation, all `docs/` subtrees,
   manuals and generated/help surfaces, designs/ADRs/studies/reports/archives,
   agent instructions, website copy, schemas, examples, and terminology in the
   CLI and source. `docs/KEY_DOCS.md:36-44` explicitly identifies embedded and
   agent-facing documentation, which is in scope but not automatically
   authoritative.
8. **Packaging and operations:** `Cargo.toml`, `Cargo.lock`,
   `rust-toolchain.toml`, `Makefile`, `manifest.scm`, `env*.sh`, installers,
   install tests, both GitHub workflows, release signing/attestation logic,
   `macos-entitlements.plist`, the committed `worksgood-pi/embedded/` bundle,
   the Pi TypeScript package, `pilot.example.toml`, scripts, runbooks, cleanup,
   observability, and service lifecycle.

### 2.2 Repository surface inventory for downstream auditors

**`[CHARTER-RULE]`** In this table, “Primary entry points” records `[FACT]`
structure at the snapshot; “Required cross-checks” prescribes downstream audit
work rather than asserting current behavior.

| Surface | Primary entry points | Required cross-checks |
|---|---|---|
| Rust package and binaries | `Cargo.toml:1-56`; `src/main.rs`; `src/bin/worksgood.rs`; `src/bin/nex.rs`; `adapters/casa/src/main.rs` | CLI help, dispatch branches, install artifacts, platform behavior |
| Library/module boundaries | `src/lib.rs:20-144`; 434 `src/**/*.rs` files; 198 `src/commands/**/*.rs` files | dependency/coupling map, public vs binary-only modules, duplicated authority |
| Graph/state/persistence | `src/graph.rs`; `src/parser.rs`; `src/atomic_file.rs`; `src/save_transaction.rs`; `src/lifecycle*.rs`; `src/work_save.rs` | state invariants, locks, atomicity, crash recovery, backward compatibility |
| Service/orchestration | `src/service/`; `src/dispatch/`; `src/attempt_runtime.rs`; `src/worker_control.rs`; completion/finalization modules | manual vs daemon flows, concurrency, worktree ownership, failure/retry paths |
| Model/execution | `src/config*.rs`; `src/profile/`; `src/executor/`; `src/commands/*handler.rs`; `src/pi_plugin/`; `src/pi_watchdog/`; `src/stream_event.rs` | route-to-process traces, credentials/fallbacks, event and usage accounting |
| Agency/chat | `src/agency/`; `src/evaluation/`; `src/function*.rs`; `src/chat*.rs`; `src/concierge.rs` | prompts, provenance, identity, assignment/evaluation gates, replayability |
| Federation/review/exec | `src/identity/`; `src/federation.rs`; `src/review/`; `src/providers/`; `src/trust.rs`; `src/commands/{identity_cmd,review_cmd,exec_fed_cmd,pilot_cmd}.rs` | ADR claims, cryptographic enforcement, trust composition, stub/deferred boundaries |
| Human/operator surfaces | `src/tui/`; `src/html.rs`; `src/commands/server.rs`; `src/notify/`; `src/telegram_commands.rs`; `pilot.example.toml` | actual human flows, help discoverability, safe defaults, diagnostics |
| Rust verification | 176 `tests/*.rs` files plus inline tests, fixtures, and snapshots | distinguish inspection from execution; map features to positive and negative evidence |
| Smoke verification | `tests/smoke/manifest.toml`; 324 `tests/smoke/scenarios/*.sh`; `tests/smoke/README.md` | owner coverage, skips, timeouts, external dependencies, human-flow fidelity |
| Install/release verification | `tests/install/`; `tests/upgrade/`; `.github/workflows/ci.yml`; `.github/workflows/release.yml` | target matrix, signing gaps, artifact contents, clean-install and upgrade behavior |
| Formal verification | `formal/` (9 Lean files at snapshot); CI formal job at `.github/workflows/ci.yml:82-125` | theorem scope, Rust/Lean conformance boundary, proof escapes and unmodeled effects |
| Pi package | `worksgood-pi/src/` (7 TypeScript files); `worksgood-pi/test/` (5 files); embedded build | source/build/embed compatibility and runtime loading; CI staleness gate at `.github/workflows/ci.yml:174-201` |
| Documentation | 603 files under `docs/`, including 554 Markdown files; 56 root-level Markdown files | authority, freshness, generated copies, duplication, archives, broken or missing indexes |
| Ancillary trees | `adapters/`, `agency/`, `examples/`, `schemas/`, `templates/`, `website/`, `terminal-bench/`, `scripts/` | determine product status before treating examples, reports, or benchmarks as normative |

**`[UNCERTAINTY]`** Counts are orientation aids, not coverage claims. Generated
files, fixtures, assets, nested packages, and files outside the named extensions
make totals non-additive.

### 2.3 Planned document tree

**`[CHARTER-RULE]`** The following names are fixed for this audit. A missing file
means its task has not landed; it must not be silently replaced by a differently
named artifact.

```text
docs/audit/2026-08-08-worksgood-system/
├── README.md
├── 10-code-architecture.md
├── 11-orchestration-lifecycle.md
├── 12-model-execution-plane.md
├── 13-agency-evaluation-chat.md
├── 14-federation-identity-security.md
├── 15-review-exec-pilot.md
├── 16-documentation-information-architecture.md
├── 17-testing-ci-quality.md
├── 18-operations-configuration-ux.md
├── 19-conceptual-model-and-vocabulary.md
├── 20-core-runtime-synthesis.md
├── 21-agency-federation-safety-synthesis.md
├── 22-product-docs-quality-synthesis.md
├── 30-contradiction-and-drift-register.md
├── 31-documentation-sync-plan.md
├── 40-system-synthesis-draft.md
├── 90-independent-review.md
└── 99-SYNTHESIS.md
```

### 2.4 Fan-out, fan-in, and provenance

**`[CHARTER-RULE]`** The planned task graph is an evidence pipeline rather than a
license to copy dependency summaries as truth:

```text
README charter
 ├─ 10 architecture ─┐
 ├─ 11 orchestration ├─> 20 core-runtime synthesis ─────────────┐
 ├─ 12 model plane ──┘                                          │
 ├─ 13 agency/chat ──┐                                          │
 ├─ 14 federation ───┼─> 21 agency/federation/safety synthesis ─┼─> 30 drift register
 ├─ 15 review/exec ──┘                                          │       └─> 31 sync plan
 ├─ 16 documentation ┐                                          │
 ├─ 17 testing/CI ───┼─> 22 product/docs/quality synthesis ─────┘
 ├─ 18 operations ───┤
 └─ 19 concepts ─────┘

20 + 21 + 22 + 30 + 31 ─> 40 draft ─> 90 independent review
40 + 90 ─> 99 final synthesis
```

**`[CHARTER-RULE]` Fan-out:** each leaf auditor reads primary repository evidence
and cites it directly; this README and dependency context are navigation, not
proof.

**`[CHARTER-RULE]` Fan-in:** a synthesis cites every input artifact, preserves
its stable finding/contradiction IDs, and spot-checks material or disputed claims
against primary evidence. It must say whether it adopts, narrows, rejects, or
leaves an input claim uncertain. Copying prose without provenance is prohibited.

**`[CHARTER-RULE]` Finalization:** `99-SYNTHESIS.md` does not supersede leaf
evidence. It links down to it, records responses to `90-independent-review.md`,
and maintains a trace from each high-severity assertion to primary evidence or
an explicit uncertainty marker.

## 3. Findings and audit contract

### 3.1 Required fractal section contract

**`[CHARTER-RULE]`** Every artifact above, and every major domain section within
a synthesis, uses these headings in this order (wording may be specialized but
not omitted):

1. **Executive abstract** — current state in plain language, most important
   findings, highest risk, confidence, and the next decision/action. State what
   was inspected and what was actually run.
2. **Scope and map** — boundaries, components, flows, dependencies, exclusions,
   and a pointer to deeper material. Include a diagram/table where relationships
   would otherwise be ambiguous.
3. **Findings** — stable IDs, labels, severity where applicable, confidence,
   concise claim, and evidence. Include positive controls and properties that
   agree, not only defects.
4. **Contradictions and drift** — code/doc, doc/doc, help/code, design/current,
   test/claim, terminology, version, and freshness conflicts. Include apparent
   contradictions resolved during checking.
5. **Risks and gaps** — impact, likelihood, affected boundary, missing evidence,
   deferred/stubbed behavior, and residual uncertainty. Test absence is a gap,
   not proof of a bug.
6. **Recommendations** — independently numbered, prioritized, scoped, linked to
   findings/contradictions, and separated into factual synchronization work,
   implementation work, and human product/design decisions.
7. **Evidence appendix** — pinned revision, exact paths/line spans, symbols,
   inspected tests, commands with date/environment/exit status, and limitations.

**`[CHARTER-RULE]`** At the synthesis level, each major subsection starts with a
short local abstract and ends with links to its local
evidence/risks/recommendations. It may link to the document-level appendix rather
than duplicate citations. Empty sections must say **“none found in the sampled
evidence”** and describe the sample; they may not disappear.

### 3.2 Statement labels and evidence classes

**`[CHARTER-RULE]`** Use the following visible prefixes. A paragraph containing
multiple classes must split them rather than applying one label to mixed prose.

| Label | Meaning | Minimum support |
|---|---|---|
| **`[FACT]`** | Observed repository fact at the pinned revision: text, type, branch, file, configuration, or static relationship. It does not assert runtime reachability. | Primary path plus line span or symbol; revision inherited from artifact header. |
| **`[VERIFIED]`** | Behavior directly exercised during this audit and observed to pass/fail as stated. | Exact command or human-flow script, date, relevant environment, exit status, and output summary. |
| **`[DOC-CLAIM]`** | A claim made by README/manual/design/help/report text. It remains a claim until source and/or execution corroborates it. | Document/help citation, date/status if present, and stated audience/authority if known. |
| **`[INFERENCE]`** | Reasoned conclusion from cited facts, verified behavior, or gaps; alternatives remain possible. | Supporting evidence, reasoning, confidence, and a falsifying check where practical. |
| **`[RECOMMENDATION]`** | Proposed change or decision, not present behavior. | Linked finding/risk, intended owner/domain, priority, and acceptance check. |
| **`[CONTRADICTION]`** | Two or more claims/evidence sources cannot all be read literally in the same scope. | Both sides cited, scope/time qualifiers, current authority if known, and resolution status. |
| **`[UNCERTAINTY]`** | Evidence is missing, ambiguous, stale, environment-dependent, or conflicting. | What is unknown, why, consequence, and the next check. |
| **`[CHARTER-RULE]`** | Normative method, structure, or instruction governing this audit; not a product-behavior claim. | This charter section; downstream deviations must be explicit and justified. |

**`[CHARTER-RULE]`** A test source that was only read supports `[FACT] test X asserts Y`, **not**
`[VERIFIED] Y works`. Likewise, compilation does not verify a CLI human flow,
and a passing happy-path test does not verify a negative security invariant.

### 3.3 Evidence hierarchy and citation contract

**`[CHARTER-RULE]`** Evidence is classified by what it can establish, not by
prestige:

1. **E1 — executed behavior:** a reproducible command or scripted human flow
   run against the pinned snapshot. Strongest evidence for that environment and
   input only.
2. **E2 — implementation:** current source, schemas, build/release configuration,
   and committed generated artifacts at the snapshot. Establishes encoded logic
   and structure, not reachability or production outcome.
3. **E3 — executable specification not run:** unit/integration/contract/smoke/
   formal test source inspected but not executed. Establishes intended assertions
   and coverage shape.
4. **E4 — normative decision/documentation:** accepted ADRs, explicit contracts,
   manuals, runbooks, embedded help, and design status. Establishes declared
   intent only; authority and status must be named.
5. **E5 — historical/contextual material:** reports, plans, incidents, archived
   docs, root notes, and commit history. Useful for provenance; never silently
   promoted to current behavior.
6. **E6 — inference or external evidence:** derived analysis, upstream docs, or
   live service observations. State assumptions, retrieval date, and why local
   primary evidence is insufficient.

**`[CHARTER-RULE]`** Prefer E1+E2 for behavior findings and E2+E3 for
implementation/coverage findings. Security claims require both enforcement-site
evidence and a negative or adversarial check where feasible. Formal claims must
delimit modeled state from filesystem, process, network, Git, and operator
effects.

**`[CHARTER-RULE]`** Use these citation forms:

- Source: ``src/parser.rs:285-395 (`load_graph`, `save_graph`, `modify_graph`)``.
- Test inspected: ``tests/integration_task_lifecycle.rs:<lines> (`<test_name>`) [inspected, not run]``.
- Documentation: ``README.md:93-119 [DOC-CLAIM; undated at snapshot]``.
- Command: a fenced exact command followed by `cwd`, revision, UTC date,
  relevant environment/tool version, exit status, and a bounded result excerpt.
- Generated CLI help: include the exact binary provenance (prefer a build from
  the pinned checkout), command, and captured help span. A globally installed
  `wg` without a hash/build link is not snapshot evidence.
- Git/history: include commit ID and exact `git log`/`git show` command; author or
  commit date does not by itself make a claim current.

**`[CHARTER-RULE]`** Line ranges are interpreted at the pinned revision. When
citing a large file, name the symbol/test/scenario as well as lines. Avoid
citations to an entire module when a smaller enforcement site exists.

### 3.4 Finding records, confidence, and risk vocabulary

**`[CHARTER-RULE]`** Every material finding uses a stable domain ID such as
`ARCH-001`, `ORCH-004`,
`FED-007`, `DOC-012`, or `X-003` (cross-cutting). It records:

- label and concise claim;
- state: **shipped/current**, **partial**, **stubbed**, **deferred**,
  **documented-only**, **historical**, **proposed**, or **unknown**;
- severity and likelihood (for a risk), plus confidence;
- affected users/data/authority boundary;
- primary evidence and counterevidence;
- owner/domain and linked recommendation/decision, if any.

**`[CHARTER-RULE]`** Severity measures plausible impact, not how surprising a
discrepancy is:

| Severity | Definition |
|---|---|
| **S0 Critical** | Credible path to broad compromise, irreversible identity/secret/data loss, unauthorized high-impact execution, or system-wide inability to complete/recover; immediate containment or release block is warranted. |
| **S1 High** | Material security/authority breach, durable corruption, major core-workflow failure, or misleading safety/release guarantee with broad impact; prioritize before the next affected release/deployment. |
| **S2 Medium** | Significant but bounded correctness, reliability, operability, upgrade, or documentation failure; workaround or limited blast radius exists. |
| **S3 Low** | Localized inconsistency, maintainability/usability issue, or minor documentation defect with low immediate impact. |
| **S4 Informational** | Observation, positive control, or improvement opportunity without a demonstrated harmful condition. |

**`[CHARTER-RULE]`** Likelihood is separate: **observed**, **likely**,
**possible**, **unlikely**, or
**unknown**. Confidence is **high** (direct, corroborated primary evidence),
**medium** (partial evidence or bounded inference), or **low** (substantial
ambiguity/missing checks). A contradiction between documents is not S0/S1
unless the resulting impact supports it.

### 3.5 Freshness and date policy

**`[CHARTER-RULE]`** All artifacts inherit the audit snapshot at the top of this
README and add an `Evidence checked through` date. Evidence uses these freshness
states:

| State | Meaning at audit time |
|---|---|
| **snapshot-current** | Static primary evidence was read at the pinned revision, or behavior was rerun against a build demonstrably from it. |
| **dated-recent** | Dated material is 90 days old or less but was not reverified; recency is not correctness. |
| **dated-aging** | 91-365 days old and not reverified. |
| **dated-stale** | More than 365 days old and not reverified. |
| **undated** | No reliable “valid as of” marker; treat authority/freshness as unknown. |
| **historical/archived** | Explicitly retrospective or archived; use only for provenance unless revalidated. |
| **superseded** | A named later authority replaces it; cite both and the superseding decision. |
| **future/proposed** | Plan, design, stub, or deferred capability; never phrase as shipped. |

**`[CHARTER-RULE]`** The repository snapshot, not a file modification timestamp,
defines static freshness. A date in prose, filename, changelog, or commit is
contextual evidence only. If the branch moves, auditors either stay pinned or
record a new revision and recheck every affected citation. External pages and
live services require a retrieval timestamp and must not silently refresh local
conclusions.

### 3.6 Explicit non-goals

**`[CHARTER-RULE]`** This audit does not:

- edit production source, tests, existing docs, schemas, workflows, packaging,
  or generated artifacts;
- certify security, cryptography, privacy, supply-chain integrity, formal
  correctness, or production readiness;
- exhaustively execute the test suite, network integrations, external model
  providers, installers on every platform, performance benchmarks, or destructive
  identity/recovery flows;
- treat `AGENTS.md`, `CLAUDE.md`, dependency summaries, filenames, test names,
  comments, or passing compilation as sole proof of behavior;
- decide contested product semantics, archive/delete documentation, choose a
  canonical vocabulary by fiat, or implement recommendations;
- infer production usage, service availability, credential safety, or release
  signing status beyond evidence gathered at the snapshot;
- conceal disagreement to make the final narrative cleaner.

## 4. Contradictions and drift

**`[CHARTER-RULE]`** The dedicated register is
`30-contradiction-and-drift-register.md`. Each entry
must contain: stable ID; claim A and claim B; exact citations; time/scope
qualifiers; evidence class; current authority or **unknown**; severity;
confidence; impact; proposed adjudication; owner/domain; and state (**open**,
**resolved**, **apparent/non-issue**, **superseded**, or **accepted debt**).
Resolved apparent contradictions remain in the register to show the check and
counter confirmation bias.

**`[FACT]`** Initial static orientation found these **triage seeds**, not final
determinations:

| ID | Record |
|---|---|
| `CHARTER-DRIFT-001` | **`[CONTRADICTION]`** `README.md:101-101` says bare `worksgood` verifies Pi and ensures the plugin, while the compiled CLI contract says an existing-graph bare launch “does not inspect Pi, plugins, profiles, concierge state, config, or services” and routes the no-option case to `run_bare` (`src/bin/worksgood.rs:11-12`, `src/bin/worksgood.rs:124-144`). Scope may differ for new vs existing repositories, but the README sentence is unqualified. **Provisional S2, medium confidence; behavior not run.** Operations/UX audit must execute both flows before selecting authority. |
| `CHARTER-DRIFT-002` | **`[CONTRADICTION]`** the root README calls Pi the sole model plane (`README.md:117-119`), while the docs quick-start still presents Claude as the default executor, `opus`/`sonnet`/`haiku` as default models, and deprecated-looking `executor` keys (`docs/README.md:152-184`). This may be legacy documentation rather than live behavior. **Provisional S2, high confidence that text conflicts; runtime authority unknown.** Model-plane and documentation audits own adjudication. |
| `CHARTER-UNCERTAINTY-001` | **`[UNCERTAINTY]`** `docs/KEY_DOCS.md:5-5` says its canonical list was last updated 2026-04-29, while this snapshot contains substantial later federation/review/exec/Pi/pilot material. No conclusion is drawn about omissions until the documentation auditor compares the full list. **S3 gap, high confidence.** |

**`[CHARTER-RULE]`** Auditors must add counterevidence and scope qualifiers rather
than copying these seeds as established product defects.

## 5. Risks and gaps

| ID | Label | Severity | Risk/gap and required control |
|---|---|---:|---|
| `CHARTER-RISK-001` | `[INFERENCE]` | S1 | A large, fast-changing cross-plane system can produce high-confidence synthesis by repeated citation of the same stale narrative. Control: leaf auditors return to E1/E2 evidence; syntheses retain provenance and independently spot-check material claims. |
| `CHARTER-RISK-002` | `[FACT]`; `[INFERENCE]` | S2 | **Fact:** the inventory found 324 smoke scripts, 176 top-level Rust test files, and 603 docs files. **Inference:** their presence can be mistaken for coverage or successful execution. Control: every test citation says **inspected** or **executed**, with skips/failures recorded. |
| `CHARTER-RISK-003` | `[INFERENCE]` | S2 | Generated CLI help, embedded Pi output, website copies, manuals, agent contracts, and source comments can drift independently. Control: compare generation/source chains and name authority; do not pick the newest-looking text without verification. |
| `CHARTER-RISK-004` | `[UNCERTAINTY]` | S2 | External providers, credentials, platform installers, signing secrets, TTY/tmux flows, and network filesystems may not be available in the audit environment. Control: record unexecuted paths and residual risk; never turn an environmental skip into a pass. |
| `CHARTER-RISK-005` | `[INFERENCE]` | S2 | Security and authority properties cross identity, trust, review, provider, lease, completion, and operator seams. Component-local tests may miss composition failures. Control: preserve cross-system sequences and negative/adversarial checks in 14, 15, 21, and 99. |
| `CHARTER-RISK-006` | `[UNCERTAINTY]` | S3 | Line citations can drift if later commits are mixed into the audit. Control: pin the revision per artifact and explicitly declare any rebase/recheck. |
| `CHARTER-GAP-001` | `[FACT]` | S4 | This charter performed static inventory only and did not run product, security, installer, integration, smoke, or human-flow tests. Downstream artifacts must not cite this README as verified behavior. |

## 6. Recommendations

1. **`CHARTER-REC-001` — `[RECOMMENDATION]` (P0, all auditors):** copy the
   snapshot metadata, seven headings, labels, severity/likelihood/confidence,
   freshness state, and evidence appendix format into every artifact. Acceptance:
   no unlabeled material claim and no missing fractal section.
2. **`CHARTER-REC-002` — `[RECOMMENDATION]` (P0, leaf auditors):** build a
   feature/claim-to-enforcement-to-test matrix and include counterevidence.
   Acceptance: major findings have primary source and a test/command status, or
   an explicit gap.
3. **`CHARTER-REC-003` — `[RECOMMENDATION]` (P0, synthesis auditors):** preserve
   stable IDs and decision status during fan-in; spot-check S0/S1, disputed, and
   cross-boundary claims. Acceptance: each adopted high-severity statement links
   to primary evidence and its leaf origin.
4. **`CHARTER-REC-004` — `[RECOMMENDATION]` (P0, drift-register owner):** record
   unresolved and resolved contradictions, with authority left `unknown` when
   evidence does not decide it. Acceptance: no conflict is “resolved” by prose
   age, filename, or majority vote alone.
5. **`CHARTER-REC-005` — `[RECOMMENDATION]` (P1, documentation/operations/test
   auditors):** compare user journeys across root README, docs indexes/manual,
   actual CLI help, source dispatch, install artifacts, and scripted human flows.
   Acceptance: each journey distinguishes claimed, reachable, and verified steps.
6. **`CHARTER-REC-006` — `[RECOMMENDATION]` (P1, roadmap owner):** separate
   factual doc synchronization from product/design decisions and production-code
   changes. Acceptance: each backlog item names its type, dependency, owner,
   evidence, acceptance check, and rollback/archive policy.
7. **`CHARTER-REC-007` — `[RECOMMENDATION]` (P1, independent reviewer):** sample
   every thematic synthesis back to primary files, look for omitted positive
   controls and counterevidence, and block release of the audit on unsupported
   S0/S1 claims. Acceptance: `90-independent-review.md` publishes its sample log
   and disposition.

## 7. Evidence appendix

### 7.1 Snapshot and inventory command

**`[VERIFIED]`** Executed from the repository worktree on 2026-08-08, before this
new audit artifact was created; revision shown below; exit status 0. This is
static inventory, not product-behavior verification. Re-running after audit
artifacts land will increase documentation counts.

```bash
pwd
uname -srm
rustc --version
cargo --version
rg --version | head -1
find --version | head -1
python3 --version
git rev-parse HEAD
git show -s --format=%cI HEAD
find src -type f -name '*.rs' | wc -l
find src/commands -type f -name '*.rs' | wc -l
find tests -maxdepth 1 -type f -name '*.rs' | wc -l
find tests/smoke/scenarios -maxdepth 1 -type f -name '*.sh' | wc -l
rg -c '^\[\[scenario\]\]' tests/smoke/manifest.toml
find docs -type f | wc -l
find docs -type f -name '*.md' | wc -l
find . -maxdepth 1 -type f -name '*.md' | wc -l
find worksgood-pi/src -type f -name '*.ts' | wc -l
find worksgood-pi/test -type f | wc -l
find formal -type f -name '*.lean' | wc -l
find .github/workflows -maxdepth 1 -type f | wc -l
```

Bounded output:

```text
cwd: /home/bot/wg/.wg-worktrees/agent-1
host: Linux 6.8.0-90-generic x86_64
rustc: 1.96.0 (ac68faa20 2026-05-25)
cargo: 1.96.0 (30a34c682 2026-05-25)
ripgrep: 15.1.0 (rev af60c2de9d)
GNU findutils: 4.9.0
Python: 3.12.3
revision: b0892ea7496fd2cc8f641417a3d8e33ca9add369
commit_time: 2026-08-07T12:38:38+02:00
src Rust files: 434
src/commands Rust files: 198
top-level Rust test files: 176
smoke scenario shell files: 324
manifest scenario blocks: 324
docs files: 603
docs Markdown files: 554
root-level Markdown files: 56
Pi source TypeScript files: 7
Pi test files: 5
formal Lean files: 9
GitHub workflow files: 2
```

**`[UNCERTAINTY]`** Limitations: counts include files regardless of
reachability/status and exclude
nested test targets from the “top-level Rust test” count. Equal smoke script and
manifest counts do not establish a one-to-one mapping without a name-level join.

### 7.2 Primary evidence sampled for this charter

**`[FACT]`** The following primary files were statically inspected; entries
explicitly say when executable behavior was not run.

| Evidence | What was observed | Class/freshness |
|---|---|---|
| `Cargo.toml:1-56` | package metadata, four binary declarations, feature boundary | E2, snapshot-current |
| `src/lib.rs:20-144` | broad exported module and re-export surface | E2, snapshot-current |
| `src/graph.rs:382-382`, `689-689`, `2705-2705` | status, task, and work-graph type locations | E2, snapshot-current |
| `src/parser.rs:285-395` | graph load/save/modify and locking/persistence boundary | E2, snapshot-current |
| `src/atomic_file.rs:1-127` | atomic replace/create and corrupt-file quarantine helpers | E2, snapshot-current |
| `src/save_transaction.rs:1-139` | pure save-transaction reducer, schema, phases, request types | E2, snapshot-current |
| `src/workgraph_dir.rs:1-68` | `.wg`/`.workgraph` directory resolution precedence | E2, snapshot-current |
| `README.md:93-119` | install/product/model-plane claims | E4, undated |
| `src/bin/worksgood.rs:11-12`, `124-151` | compiled product boundary and bare/setup dispatch | E2, snapshot-current |
| `docs/README.md:152-184` | first-time setup and legacy-looking executor/model claims | E4, undated |
| `docs/KEY_DOCS.md:1-44` | documentation index date and embedded-doc map | E4, dated-aging (101 days old at the audit date) |
| `tests/smoke/README.md:1-101` | documented smoke owner, exit, cleanup, live, and assertion contracts | E4/E3, snapshot-current text; not executed |
| `tests/smoke/manifest.toml:1-17` | manifest contract header | E3, snapshot-current; scenarios not executed |
| `.github/workflows/ci.yml:11-220` | check/build/formal/integration/Windows/Pi/nightly job structure | E2, snapshot-current; workflow not executed here |
| `.github/workflows/release.yml:1-209`, `579-660` | release inputs, five-target matrix, build, signing, and assembly surfaces | E2, snapshot-current; release not executed here |
| `Makefile:1-29` | Pi embed/check and patched-Pi convenience targets | E2, snapshot-current |
| `worksgood-pi/package.json:1-47` | Pi package identity, build/test scripts, peer dependencies | E2, snapshot-current |
| `pilot.example.toml:1-76` | operator-supplied pilot inputs and declared safe defaults | E2/E4, snapshot-current; pilot not executed |
| `rust-toolchain.toml:1-20` | pinned toolchain policy and version | E2/E4, snapshot-current |

### 7.3 Commands not run for this charter

**`[FACT]`** No `cargo build`, `cargo test`, smoke scenario, installer, release, network,
identity, provider, pilot, TUI, browser, or external-model command was run.
Their presence in source/CI is `[FACT]`; their behavior remains unverified by
this artifact.
