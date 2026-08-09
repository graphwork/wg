# Independent review of the WorksGood system-audit synthesis

**Review date:** 2026-08-09

**Audit under review:** [`40-system-synthesis-draft.md`](40-system-synthesis-draft.md)

**Audited product snapshot:** `b0892ea7496fd2cc8f641417a3d8e33ca9add369`

**Review checkout:** `04676691472f5890f7d5d4f2992d8ec468850517`

**Evidence checked through:** 2026-08-09

**Status:** independent audit review; no production source, tests, or pre-existing documentation changed

**Normative method:** [`README.md`](README.md)

## 1. Executive abstract

**`[FACT]`** The draft is broad, well linked, unusually candid about partial systems, and substantially grounded in direct source and bounded inherited executions. Independent static checks at the pinned product snapshot supported the central findings sampled across product boundary, persistence, lifecycle, model execution, agency/evaluation, federation, review, remote execution/Pilot, testing/CI, documentation, and operations. The draft also preserves positive controls and apparent non-issues instead of presenting an all-defect narrative (`40-system-synthesis-draft.md:40-96,512-573,654-768`).

**`[INFERENCE]` (high confidence)** The draft is a strong review candidate, but it is not release-ready as the final audit. Its largest problem is calibration and method conformance, not wholesale factual failure. It repeatedly uses **confidence** where the charter requires **likelihood**, escalates at least one explicit S2 uncertainty into an executive S1 current mechanism, mixes executed behavior with static snapshot inference under one label, and calls snapshot facts “current” after material post-snapshot product changes. Those defects make the ranking harder to trust than the underlying evidence.

**`[FACT]`** The draft also omits the charter-required per-domain **Scope and map** and **Contradictions and drift** headings. It substitutes `OBSERVED FACT` and `VERIFIED BEHAVIOR` for the charter's prescribed visible prefixes without recording a justified deviation (`README.md:196-244`; draft heading inventory at `40-system-synthesis-draft.md:145-504`). This is not cosmetic: the missing local maps make it hard to see what was not sampled in each domain.

**`[RECOMMENDATION]` — release recommendation:** **DO NOT publish `99-SYNTHESIS.md` unchanged from this draft. Conditional release after the blocking corrections in §6.1.** A full re-audit is not required. The final author should preserve the draft's architecture and primary conclusions, correct evidence labels and risk calibration, add explicit coverage gaps and snapshot applicability, and narrow the few security/operations statements identified below.

### Overall score

**75/100 — substantial, useful, and evidence-rich; blocked on calibration, charter conformance, and present-day applicability.**

| Rubric dimension | Weight | Score | Independent judgment |
|---|---:|---:|---|
| Repository/domain coverage | 15 | 13 | All three thematic synthesis families and nearly all major draft domains are represented; human UI/channel, formal-model boundary, ancillary adapter, and performance coverage are too implicit. |
| Factual support and traceability | 20 | 16 | Most sampled claims resolve to direct source. Strong artifact crosswalk. Several `[VERIFIED BEHAVIOR]` statements require leaf traversal for the actual command/environment, and one mixes installed-binary behavior with snapshot source. |
| Calibration: severity, likelihood, confidence, maturity | 15 | 7 | Confidence is often substituted for likelihood; some S1 ratings exceed the charter definition or conflict with the register; `current/shipped` is too strong for a now-historical snapshot. |
| Internal consistency | 10 | 7 | Main architecture is coherent, but the head-of-line finding is S2 uncertainty in the register and S1 current in the draft; mixed labels and snapshot vocabulary also conflict with the charter. |
| Security-claim discipline | 10 | 8 | The draft correctly rejects security certification and identifies enforcement gaps. Inbox quota/retention and custody-at-rest countercontrols need to travel with the executive security claims. |
| Counterevidence and positive controls | 10 | 8 | Strong resolved/non-issue list and positive-control coverage. Deliberate extension behavior, bounded inbox controls, and Pilot's own honest CLI caveat are underweighted. |
| Navigability/fractal usability | 8 | 7 | Excellent TOC, local abstracts, deep links, and artifact matrix; missing local scope/contradiction headings and ID density slow a stop-at-any-depth reader. |
| Usefulness for a human documentation-sync process | 7 | 6 | The roadmap separates F/D/I/S/V and supplies dependency order. It still needs one compact disposition table tying executive issues to exact claim-containment edits, decisions, and verification owners. |
| Audit-method compliance | 5 | 3 | Snapshot and limits are explicit, but required labels/headings, likelihood, and mixed-evidence separation do not conform. |
| **Total** | **100** | **75** | **Conditional, not releasable unchanged.** |

## 2. Scope and map

### 2.1 Material reviewed

**`[FACT]`** This review read in full:

- the charter, `README.md`;
- the draft, `40-system-synthesis-draft.md`;
- the deduplicated register, `30-contradiction-and-drift-register.md`;
- the synchronization roadmap, `31-documentation-sync-plan.md`.

**`[FACT]`** Risk-based primary checks were made against the pinned product snapshot using `git show`, `git grep`, `git ls-tree`, and `git diff`. The sample deliberately crossed every thematic synthesis family:

```text
20 core runtime
  -> product/types -> persistence -> lifecycle -> model execution
21 agency/federation/safety
  -> agency/evaluation -> identity/custody -> transport -> review -> remote/Pilot
22 product/docs/quality
  -> CI/smoke -> documentation/concepts -> operations/config/IPC/accounting
```

The sample log is in §3.2 and exact commands are in §7.2.

### 2.2 Review boundaries

**`[FACT]`** No production source, test, workflow, schema, generated output, or pre-existing document was edited. This review adds only this file.

**`[UNCERTAINTY]`** This reviewer did not rebuild and execute the entire pinned snapshot. Product behavior labeled `[VERIFIED]` below is limited to repository/provenance commands; runtime results inherited by the draft remain inherited evidence and were checked for source plausibility, not rerun. Static inspection can confirm encoded gates and missing calls, not runtime reachability in every environment.

**`[FACT]`** The review checkout is materially later than the audited product snapshot: the exact command in §7.2 reported **89 non-audit files changed, 5,995 insertions, and 413 deletions** between `b0892ea7` and review HEAD. Relevant intervening commits include setup activation/readiness, Pi accounting/review visibility, and local completion/recovery work. Therefore this is a review of a pinned historical system audit, not certification of review-HEAD behavior.

## 3. Findings

### 3.1 Scored finding summary

#### `IR-001` — the synthesis has high coverage and strong evidence navigation

- **Label/state:** `[FACT]`; draft property.
- **Severity:** S4 informational. **Likelihood:** observed document property. **Confidence:** high.
- **Evidence:** the draft links every existing numbered artifact `10`–`31`, provides representative primary spans, lists inherited executions by leaf, and states explicit evidence limits (`40-system-synthesis-draft.md:654-768`).
- **Counterevidence:** the artifact table is not itself a coverage proof, and local domain sections omit the charter's scope/map heading.
- **Disposition:** preserve the TOC, local abstracts, artifact matrix, and final evidence-limit section.

#### `IR-002` — central factual claims survived a broad independent static sample

- **Label/state:** `[FACT]`; snapshot-current static support.
- **Severity:** S4 positive control. **Likelihood:** observed in the static sample. **Confidence:** high for encoded source shape.
- **Evidence:** §3.2. Eleven of thirteen checks support the draft as written; two support it only after narrowing/counterevidence.
- **Disposition:** no wholesale rewrite or reversal is warranted.

#### `IR-003` — the draft violates the charter's fractal section and label contract

- **Label/state:** `[CONTRADICTION]`; open.
- **Severity:** S2. **Likelihood:** observed charter/draft conflict. **Confidence:** high.
- **Claim A:** every artifact and major synthesis domain must use abstract, scope/map, findings, contradictions/drift, risks/gaps, recommendations, and evidence appendix in that order; visible prefixes are prescribed (`README.md:196-244`).
- **Claim B:** sections 3–11 use Abstract → Findings → Risks → Recommendations → Deeper artifacts, with no local Scope/map or Contradictions/drift headings; the draft systematically uses `OBSERVED FACT` and `VERIFIED BEHAVIOR` (`40-system-synthesis-draft.md:145-504`).
- **Impact:** omissions and counterevidence are harder to locate at a local stopping depth, and evidence classes cannot be mechanically checked against the charter.
- **Disposition:** blocking correction `BR-1`.

#### `IR-004` — risk records confuse confidence with likelihood

- **Label/state:** `[CONTRADICTION]`; open.
- **Severity:** S2 audit-integrity issue. **Likelihood:** observed in the draft. **Confidence:** high.
- **Claim A:** severity, likelihood, and confidence are separate mandatory fields (`README.md:287-313`).
- **Claim B:** the executive table explicitly labels its column “Severity / confidence,” and domain risks repeatedly use forms such as `S1/high`, where “high” is usually confidence rather than likelihood (`40-system-synthesis-draft.md:59-72,129-131,234-240,361-368,405-410,488-493`).
- **Impact:** a high-confidence static discrepancy can be read as a highly likely severe incident. This particularly distorts documentation, accounting, learning, and CI findings.
- **Disposition:** blocking correction `BR-2`; do not mechanically add `likely`—adjudicate it.

#### `IR-005` — the head-of-line finding is escalated beyond its register authority

- **Label/state:** `[CONTRADICTION]`; open.
- **Severity:** S2 audit-integrity issue. **Likelihood:** observed draft/register conflict. **Confidence:** high.
- **Claim A:** `WGDR-U04` records an installed-binary trace with unknown audited-build identity as **S2 open uncertainty**, medium applicability, requiring a build-ID candidate concurrency scenario (`30-contradiction-and-drift-register.md:191`).
- **Claim B:** the draft makes it executive priority 1 as a “current mechanism” and **S1 / medium**, then repeats an S1 risk (`40-system-synthesis-draft.md:63,229,236`).
- **Inference:** pinned source makes blocking plausible, but source shape does not independently establish duration, all-request impact, or S1 likelihood at the exact snapshot.
- **Disposition:** blocking correction `BR-3`: restore S2/open uncertainty or add exact candidate-built E1 evidence and a reasoned S1 impact argument.

#### `IR-006` — evidence classes are sometimes mixed in one labeled statement

- **Label/state:** `[CONTRADICTION]`; open.
- **Severity:** S2. **Likelihood:** observed mixed-label construction. **Confidence:** high.
- **Evidence:** the charter requires mixed classes to be split (`README.md:229-244`). Draft §5 finding 5 is labeled verified behavior but says the exact snapshot is *not* verified, then appends a static source observation (`40-system-synthesis-draft.md:229`). Similar “verified + causal source correction” bundling appears in Pi accounting (`40-system-synthesis-draft.md:268-270`).
- **Impact:** readers cannot tell which clause was executed against which binary.
- **Disposition:** blocking correction `BR-4`: split `[VERIFIED] installed binary …`, `[UNCERTAINTY] snapshot applicability …`, and `[FACT] source shape …`; link the exact leaf command.

#### `IR-007` — snapshot applicability is explicit globally but misleading locally

- **Label/state:** `[FACT]` + `[INFERENCE]`; open presentation issue.
- **Severity:** S2 for documentation-sync consumers. **Likelihood:** likely if used as a current backlog. **Confidence:** high.
- **Evidence:** the draft header pins `b0892ea7` and its caution scopes facts to cited revisions (`40-system-synthesis-draft.md:5-15,768`). It nevertheless defines and repeatedly uses `current/shipped` (`40-system-synthesis-draft.md:121-125`) rather than the charter's `snapshot-current`. Review HEAD differs in 89 non-audit files, including several remediations (§2.2, §7.2).
- **Impact:** a human synchronization process can reopen already changed behavior or publish historical defects as present tense.
- **Disposition:** blocking correction `BR-5`: replace local “current” with “snapshot-current” where appropriate and add a post-snapshot applicability notice. Do not silently refresh selected findings.

#### `IR-008` — security findings are directionally correct but two countercontrols need to travel with them

- **Label/state:** `[FACT]` + `[INFERENCE]`; partial.
- **Severity:** S2 audit-presentation issue. **Likelihood:** possible misreading. **Confidence:** high.
- **Evidence:** same-process key loading and unauthenticated inbox operations are directly encoded (§3.2 checks S6–S7). Counterevidence: custody can encrypt seeds under a KEK, though it still does not isolate same-UID invocation (`src/identity/keys.rs:226-300,340-377` at `b0892ea7`); inbox writes are capped by per-inbox count/bytes and retention logic, though unauthorized read/delete and quota consumption remain (`src/identity/node.rs:408-443,551-572` at `b0892ea7`).
- **Impact:** “fill an inbox” can be read as unbounded resource exhaustion, and “plaintext” can be read as unconditional. Neither narrowing removes the S1 authentication/custody concern.
- **Disposition:** blocking correction `BR-6`: say **consume the bounded quota** and **same-user in-process custody, with optional at-rest KEK but no hostile-worker isolation**.

#### `IR-009` — some operational findings omit design intent that matters to adjudication

- **Label/state:** `[FACT]`; open counterevidence gap.
- **Severity:** S3. **Likelihood:** observed counterevidence omission. **Confidence:** high.
- **Evidence:** `config set` deliberately accepts unknown paths “so EVERY knob is reachable” and deserializes the resulting document as `Config`, while `toml::to_string_pretty` still erases comments and unknown keys can be ineffective (`src/commands/config_cmd.rs:3029-3098` at `b0892ea7`). The draft presents only silent destruction/typo persistence in its executive ranking (`40-system-synthesis-draft.md:66,477-482`).
- **Impact:** a factual defect can be “fixed” by banning a deliberate extension surface without the product decision requested by roadmap `I-CONTROL-INTEGRITY` (`31-documentation-sync-plan.md:333`).
- **Disposition:** non-blocking if the final explicitly carries the roadmap's extension-namespace decision; otherwise include in `BR-2` recalibration.

#### `IR-010` — the roadmap is structurally useful but too dense for first human action

- **Label/state:** `[INFERENCE]`; current usability gap.
- **Severity:** S3. **Likelihood:** likely for a new maintainer. **Confidence:** medium-high.
- **Evidence:** the roadmap separates factual, decision, implementation, structural, and verification work; supplies six phases, owner domains, acceptance, rollback, and decision queue (`31-documentation-sync-plan.md:74-204,276-484`). The draft condenses this but still requires navigation among executive priorities, `WGDR-*`, `DEC-*`, and phase IDs (`40-system-synthesis-draft.md:581-644`).
- **Falsifier:** a new maintainer can select one top issue and identify the exact immediate doc containment, human decision, implementation owner, evidence check, and current disposition without searching four files.
- **Disposition:** non-blocking improvement `NBR-3`: add one compact handoff matrix, not another narrative.

### 3.2 Primary-evidence spot checks

All source spans below are at `b0892ea7496fd2cc8f641417a3d8e33ca9add369`. “Supported” means the encoded source/test/doc shape supports the bounded draft claim; it is not an assertion that a runtime path was independently executed.

| Check / thematic domain | Draft claim sampled | Direct primary evidence | Independent result |
|---|---|---|---|
| `S1` Product identity/boundary | Local durable center; multiple binaries and setup-neutral existing-graph launcher | `Cargo.toml:23-41`; `src/bin/worksgood.rs:6-16,124-151` | **Supported.** Four binaries are declared, and bare existing-graph launch routes to `run_bare` without advanced setup. Casa's public support status remains a decision. |
| `S2` Architecture/persistence | Strong Unix serialization with explicit non-Unix gap | `src/parser.rs:83-157,275-357` | **Supported, bounded.** Unix uses `flock`; non-Unix lock methods are no-ops. Temp file is flushed/fsynced before rename. No parent-directory fsync is visible in the sampled span. |
| `S3` Lifecycle/completion | Done help exposes rejected flags; manual claim omits pause/time gates | `src/cli.rs:528-554`; `src/main.rs:1261-1274`; `src/commands/claim.rs:18-90` | **Supported.** Parser/dispatch contradiction is exact. Claim checks dependency disposition and status but not pause or scheduled time in the sampled admission body. |
| `S4` Model execution/accounting | Normal Pi JSON workers differ from hermetic RPC handler; v3 receipt/commit omitted usage | `src/service/executor.rs:1729-1752`; `src/commands/pi_handler.rs:492-534`; no `token_usage` match in `completion_done.rs`, `completion_review_model.rs`, or `completion_review.rs` | **Supported statically.** The worker argv lacks `-e/-ne`; RPC includes them. The source search supports the accounting omission but not the inherited end-to-end trace by itself. |
| `S5` Agency/evaluation/evolvability | Modern completion review does not feed legacy agency learning; compact receipt is thin | `src/completion_review.rs:83-121`; `src/agency/evolver.rs:120-224`; `git grep` found `record_evaluation*` definitions/tests but no completion caller | **Supported statically.** Evolver counts `agency/evaluations/*.json`; receipt has exact binding/verdict/model/time but no usage, attempt, latency, or source composition. “Statistically empty” remains environment-dependent. |
| `S6` Federation identity/custody | Same-user/in-process custody falls short of hostile-worker signer boundary | `src/identity/keys.rs:51-68,226-300,340-377`; `src/identity/sigchain.rs:493-515,884-925` | **Supported with required counterevidence.** `sign_digest` loads the seed in process. KEK can protect at rest; warning is opt-in. Recovery window checks signer-carried `recovery_at`, not verifier wall time. |
| `S7` Federation transport | Inbox list/get/delete is unauthenticated; overwrite/quota risks exist | `src/identity/node.rs:408-443,551-572`; `src/identity/transport.rs:318-354,480-496` | **Supported but narrow “fill.”** Routes show no recipient-auth input. A repeated ID overwrites; a new ID is bounded by count/byte limits. Unauthorized quota consumption/read/delete remain. |
| `S8` Review safety/audit | Verdict log is unsigned/best-effort and deterministic `n` is ignored | `src/review/verdict.rs:53-80,117-190`; `src/review/pass2_review.rs:80-98`; ignored results at `src/commands/{exec_fed_cmd.rs:1164,identity_cmd.rs:1332,trace_import.rs:118}` | **Supported.** Countercontrol: record RMW is lock-protected, CID-stamped, and atomically written. `load_chain` in the sampled span parses but does not revalidate links/CIDs. |
| `S9` Remote execution/Pilot | Planner can select remote, normal spawn rejects it, real Pilot is bootstrap | `src/dispatch/plan.rs:583-640`; `src/commands/spawn_task.rs:330-347`; `src/commands/pilot_cmd.rs:43-50,1066-1125,1184-1215` | **Supported with counterevidence.** Real Pilot records `check_passed: None`; dry-run uses fixed output. Its CLI itself tells the operator the full check waits for both hosts, so the overclaim is primarily documentation/product naming, not silent CLI success. |
| `S10` Testing/CI/release | Large integration estate is weakly selected; smoke-policy classes conflict | `.github/workflows/ci.yml:68-201`; 176 top-level `tests/*.rs` paths; `tests/smoke/README.md:1-29,82-87`; `release_workflow_signing_contract.sh:1-18` | **Supported.** CI runs library, selected binary/formal/canary, and `integration_service`, not the full integration estate. Smoke policy requires live binaries while a declared static contract scenario exists. “Not selected” must not be read as failing. |
| `S11` Documentation/concepts | Manual/source DAG and canonical index are ambiguous | `docs/manual/README.md:30-42`; `scripts/sync-docs.sh:1-8,77-118`; `docs/KEY_DOCS.md:1-16` | **Supported.** Unified manual is called authoritative while chapter Typst files drive generation; failed conversion can copy raw Typst to `.md`; KEY_DOCS calls itself canonical and COMMANDS complete. |
| `S12` Operations/config/accounting | Config edit is lossy; spend day and metrics scope are misleading | `src/commands/config_cmd.rs:3029-3098`; `src/commands/spend.rs:27-57`; `src/metrics.rs:1-29`; `src/commands/metrics.rs:1-34` | **Supported with intent caveat.** Pretty serialization is lossy; all tasks are assigned invocation day; cleanup counters are process-local statics. Severity/likelihood need recalibration. |
| `S13` Worker-control operations | Array response can fail after message cursor mutation | `src/commands/service/ipc.rs:253-275,720-790`; `src/messages.rs:631-696` | **Static mechanism supported; runtime result inherited, not rerun.** `data` is flattened; message/artifact operations return arrays; `read_unread` writes cursor/status before response serialization. |

### 3.3 Missing or underrepresented counterevidence

**`[FACT]`** The draft does retain a valuable resolved/non-issue list (`40-system-synthesis-draft.md:545-557`). The following additional controls should accompany, not erase, the corresponding defects:

1. **Custody:** optional KEK provides at-rest encryption; the defect is requester/process/UID isolation, not unconditional plaintext (`src/identity/keys.rs:226-300`).
2. **Inbox:** per-inbox count/byte limits bound storage exhaustion; unauthorized read, delete, overwrite, and quota consumption remain (`src/identity/node.rs:551-572`).
3. **Review log:** exclusive locking and atomic write reduce lost-update/torn-write risk even though the log is unsigned and callers can ignore failure (`src/review/verdict.rs:136-190`).
4. **Config:** accepting unknown paths is deliberate extensibility, not solely a typo bug; ineffective keys and comment erasure are still real (`src/commands/config_cmd.rs:3029-3098`).
5. **Pilot:** the real CLI output states that the full check requires both hosts; misleading “turnkey/live” claims should be attributed to product/runbook language rather than to a fabricated `check_passed=true` (`src/commands/pilot_cmd.rs:1184-1215`).

## 4. Contradictions and drift

### 4.1 Draft-to-register contradictions

| Review ID | Draft | Register/charter | Status |
|---|---|---|---|
| `IR-C01` | Completion-review blocking is S1/current (`40`:63,229,236). | `WGDR-U04` is S2/open uncertainty with medium snapshot applicability (`30`:191). | **Open; blocking.** |
| `IR-C02` | Risk notation combines severity/confidence (`40`:59-72 and domain risk lists). | Charter requires severity, likelihood, and confidence separately (`README`:287-313). | **Open; blocking.** |
| `IR-C03` | Domain sections omit scope/map and contradictions/drift and use alternate prefixes. | Charter makes section order and visible prefixes normative (`README`:196-244). | **Open; blocking.** |
| `IR-C04` | `current/shipped` is used for the audited snapshot. | Charter supplies `snapshot-current`; review HEAD has material post-snapshot product changes. | **Open; blocking for final applicability.** |

### 4.2 Apparent contradictions independently confirmed as non-issues

**`[FACT]`** The following register safeguards survived the sample and should not be “fixed away”:

- route-free bare/new graph versus configured execution are different scopes (`WGDR-R01`);
- discovery and unattended admission may legitimately differ (`WGDR-R02`);
- author trust fails closed to Unknown and provider trust can only lower it (`src/trust.rs:84-124`; `WGDR-R04`);
- offline static-key forward-secrecy absence is disclosed debt, not a hidden implementation drift (`WGDR-R06`);
- completion review and agency performance evaluation are distinct stores/authorities (`WGDR-R10`).

### 4.3 Revision drift

**`[FACT]`** Register and roadmap checks were production-byte-equivalent to `b0892ea7` at their stated revisions: direct `git diff --name-only b0892ea7..4b8d3cb4` and `..e7e58501`, excluding this audit directory, each returned zero paths. That supports their fan-in provenance.

**`[FACT]`** The final-review checkout is no longer product-byte-equivalent (§2.2). The audit can remain pinned, but a final document released now must not use unqualified present tense for snapshot findings.

## 5. Risks and coverage gaps

### 5.1 Coverage omissions that the final must make explicit

These are evidence gaps, not proof of defects.

| Gap | What is underrepresented in the draft | Required final treatment |
|---|---|---|
| Human interaction surfaces | TUI, HTML/server, browser flow, Telegram, Matrix, notifications, accessibility, and terminal discoverability receive little direct synthesis despite charter scope (`README.md:82-88`). | Add a coverage-limit row naming what was inspected/run and point to leaf evidence; do not imply the “human-facing” layer was broadly verified. |
| Formal verification | Formal checks appear mainly as a positive CI control and limitation; theorem-to-Rust scope is not summarized for a stop-at-§9 reader. | State the modeled lifecycle boundary and excluded filesystem/process/network/operator effects in §9. |
| Ancillary/product boundaries | Casa adapter, `website/`, schemas/examples/templates, terminal benchmark, and ancillary scripts are inventory items more than analyzed product surfaces. | Mark sampled/not deeply adjudicated; retain the Casa packaging decision. |
| Performance and scale | No sustained large-graph, high-concurrency, latency, disk-pressure, or long-duration service campaign. | Add explicit uncertainty; do not infer scalability from locking or smoke inventory. |
| Supply chain/dependency security | Release signing and embedded Pi staleness are sampled, but dependency vulnerability, npm/cargo provenance, and compromise response are not. | State non-coverage; avoid “supply-chain ready” implications. |
| Destructive and cross-platform behavior | Power loss, disk full, NFS, Windows/macOS runtime, real external providers, distinct-UID custody, and real two-host Pilot remain unverified. | Preserve and elevate the existing §14 limitations to the executive coverage map. |
| Human synchronization ownership | Roadmap names roles, not people; no CODEOWNERS-like file was found by the roadmap. | Keep Phase 0 owner assignment as a precondition; do not call the plan dispatch-ready. |

### 5.2 Risks if the draft is released unchanged

1. **`[INFERENCE]`** A documentation team may treat historical `current/shipped` labels as the state of review HEAD and redo or contradict post-snapshot fixes.
2. **`[INFERENCE]`** S1 volume without likelihood will flatten priority: custody/inbox authentication competes on the same visual plane as comment preservation, accounting labels, agency-learning disconnection, and unselected tests.
3. **`[INFERENCE]`** Mixed executed/static labels can overstate source-built reproduction, especially for daemon blocking and worker IPC.
4. **`[INFERENCE]`** Missing local coverage maps can make thinly sampled UI, formal, platform, and ancillary surfaces disappear from a fractal reader's view.
5. **`[INFERENCE]`** Security narrowing without the named countercontrols can induce either alarmism or an overly broad fix that removes intentional bounded behavior.

## 6. Recommendations

### 6.1 Blocking corrections before final release

1. **`BR-1` — `[RECOMMENDATION]` (P0, final-synthesis owner): restore charter conformance.** For every major domain, add explicit local **Scope and map** and **Contradictions and drift** subsections, even if concise. Use the charter's exact `[FACT]`, `[VERIFIED]`, `[DOC-CLAIM]`, `[INFERENCE]`, `[RECOMMENDATION]`, `[CONTRADICTION]`, and `[UNCERTAINTY]` prefixes, or explicitly record and justify a charter deviation. Acceptance: automated heading/label inventory plus human check against `README.md:196-244`.
2. **`BR-2` — `[RECOMMENDATION]` (P0, final-synthesis + register owners): rescore material risks.** Add separate severity, likelihood, and confidence for each executive/material risk. Re-evaluate at least configuration comment loss/unknown keys, agency-learning disconnect, setup/doctor drift, spend/metrics, CI selection, and public metadata exposure against the charter's S1 definition. Acceptance: no `S1/high` shorthand; every S1 states a concrete broad-impact path and scope.
3. **`BR-3` — `[RECOMMENDATION]` (P0, orchestration owner): reconcile daemon blocking.** Either restore `WGDR-U04`'s S2/open-uncertainty classification in the executive summary or supply exact candidate-built build identity, command, environment, exit/result, concurrent operations affected, and an S1 impact argument. Acceptance: draft, register, and final use one state/severity.
4. **`BR-4` — `[RECOMMENDATION]` (P0, evidence editor): split mixed evidence classes.** In particular, separate the installed-daemon observation, pinned-source shape, and snapshot uncertainty; likewise separate the Pi wrapper execution from causal source inspection. Acceptance: every `[VERIFIED]` clause links to exact command/date/environment/exit and does not contain an unverified clause.
5. **`BR-5` — `[RECOMMENDATION]` (P0, final-synthesis owner): make applicability unmistakable.** Use `snapshot-current` rather than unqualified `current`; add a top-level “not current-HEAD” banner and a bounded post-snapshot delta note naming affected domains without selectively closing findings. Acceptance: a reader cannot mistake the report for behavior at `04676691`.
6. **`BR-6` — `[RECOMMENDATION]` (P0, security reviewer): narrow and balance security statements.** Carry optional at-rest KEK alongside the same-user custody gap; carry inbox count/byte limits alongside unauthorized quota consumption/read/delete/overwrite; retain review-log locking/atomicity alongside unsigned/best-effort gaps; attribute Pilot overclaim to the correct prose/product scope. Acceptance: each security S1 has enforcement site, adversarial gap, countercontrol, likelihood, and unverified boundary.
7. **`BR-7` — `[RECOMMENDATION]` (P0, final-synthesis owner): publish explicit coverage omissions.** Incorporate §5.1 into the executive coverage map and relevant domain sections. Acceptance: every charter surface is either synthesized, linked as sampled, or named as unreviewed/underreviewed.

### 6.2 Non-blocking improvements

1. **`NBR-1` — `[RECOMMENDATION]`: add finding state filters.** Provide a compact view for `snapshot-current defect`, `open decision`, `uncertainty`, `accepted debt`, `resolved guard`, and `post-snapshot applicability unknown`.
2. **`NBR-2` — `[RECOMMENDATION]`: add an evidence-strength glyph/key to the executive table.** Distinguish exact executed candidate, installed-binary observation, source-only, inspected test, and document claim without making the reader descend to §14.
3. **`NBR-3` — `[RECOMMENDATION]`: add one human handoff matrix.** For each executive priority: `WGDR ID -> immediate claim-containment path -> decision ID -> implementation domain -> verification command/class -> owner/status`. Reuse roadmap data rather than add prose.
4. **`NBR-4` — `[RECOMMENDATION]`: reduce repeated absolute language.** Prefer “the sampled ordinary path,” “the pinned source encodes,” and “no caller found in the searched completion modules” over universal negatives.
5. **`NBR-5` — `[RECOMMENDATION]`: provide commit-permalink citation generation.** Current `path:line` links will drift as main changes. A generated evidence index should preserve `b0892ea7` permalinks or extract hashes.
6. **`NBR-6` — `[RECOMMENDATION]`: retain resolved counterexamples near recommendations.** This will prevent future doc-sync work from banning route-free init, discovery-only handlers, offline static-key operation, or separate agency/runtime identities.

### 6.3 Explicit release recommendation

**`[RECOMMENDATION]` — HOLD.** Do not release `99-SYNTHESIS.md` if it merely copies or cosmetically edits `40-system-synthesis-draft.md`.

**Release after:** `BR-1` through `BR-7` are visibly dispositioned in the final, with either correction or an explicit reasoned rejection. No new product implementation is required to release the audit; truthful narrowing and uncertainty are acceptable.

**No need to block on:** resolving all 49 `WGDR-*` product issues, implementing the proposed registries, rerunning the full test/smoke suite, or deciding every `DEC-*`. The audit's job is to report those open states accurately, not make them disappear.

## 7. Evidence appendix

### 7.1 Environment

**`[VERIFIED]`** On 2026-08-09 UTC, the review commands ran from `/home/bot/wg/.wg-worktrees/agent-27` on Linux `6.8.0-90-generic x86_64`, review checkout `04676691472f5890f7d5d4f2992d8ec468850517`, with Rust/Cargo `1.96.0`. Repository/provenance commands completed with exit 0 unless otherwise noted.

### 7.2 Exact command log

```bash
# Required artifact inventory and size
find docs/audit/2026-08-08-worksgood-system -maxdepth 1 -type f -printf '%f\n' | sort
wc -l docs/audit/2026-08-08-worksgood-system/*.md

# Snapshot/review relation
git rev-parse HEAD
git merge-base --is-ancestor b0892ea7496fd2cc8f641417a3d8e33ca9add369 HEAD
git diff --stat b0892ea7496fd2cc8f641417a3d8e33ca9add369..HEAD \
  -- . ':(exclude)docs/audit/2026-08-08-worksgood-system/**'
git log --format='%h %s' \
  b0892ea7496fd2cc8f641417a3d8e33ca9add369..HEAD \
  -- src/commands/completion_done.rs src/commands/config_cmd.rs \
     src/commands/setup.rs src/commands/service/ipc.rs

# Register/roadmap production-byte equivalence at their stated revisions
for r in 4b8d3cb45475c71de6b76d44f3215e365c7e75a6 \
         e7e58501ff13be8fccbb71ee4f1bf343bff56fea; do
  git diff --name-only b0892ea7496fd2cc8f641417a3d8e33ca9add369..$r \
    -- . ':(exclude)docs/audit/2026-08-08-worksgood-system/**'
done

# Draft structure/labels/risk inventory
rg -n '^## |^### ' \
  docs/audit/2026-08-08-worksgood-system/40-system-synthesis-draft.md
rg -o '`\[(OBSERVED FACT|FACT|VERIFIED BEHAVIOR|VERIFIED|DOC-CLAIM|INFERENCE|RECOMMENDATION|CONTRADICTION|UNCERTAINTY)[^]]*\]`' \
  docs/audit/2026-08-08-worksgood-system/40-system-synthesis-draft.md | sort | uniq -c
rg -n 'S[01]( |/|–|-)|Severity' \
  docs/audit/2026-08-08-worksgood-system/40-system-synthesis-draft.md

# Pinned primary-source checks (representative form; every table row used this form)
rev=b0892ea7496fd2cc8f641417a3d8e33ca9add369
git show $rev:src/cli.rs | nl -ba | awk 'NR>=528&&NR<=557 {print}'
git show $rev:src/main.rs | nl -ba | awk 'NR>=1255&&NR<=1280 {print}'
git show $rev:src/identity/keys.rs | nl -ba | \
  awk 'NR>=51&&NR<=68 || NR>=226&&NR<=300 || NR>=340&&NR<=377 {print}'
git show $rev:src/identity/node.rs | nl -ba | \
  awk 'NR>=408&&NR<=443 || NR>=551&&NR<=572 {print}'
git show $rev:src/review/verdict.rs | nl -ba | \
  awk 'NR>=53&&NR<=80 || NR>=117&&NR<=190 {print}'
git grep -n -E 'record_evaluation|record_evaluation_with_inference' $rev \
  -- src/commands/completion_done.rs src/commands/completion_submit.rs \
     src/completion_review.rs src/evaluation src/agency
git ls-tree -r --name-only $rev tests | \
  awk '/^tests\/[^\/]+\.rs$/{n++} END{print n+0}'
git show $rev:.github/workflows/ci.yml | nl -ba | awk 'NR>=68&&NR<=201 {print}'
```

Bounded results used in this review:

- review checkout is a descendant of the pinned snapshot;
- register and roadmap revisions had zero non-audit production/pre-existing-doc differences from `b0892ea7`;
- review HEAD differs from the snapshot in 89 non-audit files (`5,995 insertions`, `413 deletions`);
- pinned tree contains 176 top-level Rust integration targets;
- source excerpts matched the spans and conclusions recorded in §3.2.

### 7.3 Validation commands for this artifact

**`[VERIFIED]`** Executed from `/home/bot/wg/.wg-worktrees/agent-27` on
2026-08-09 UTC after writing this artifact. Every command below exited 0.
`test -s` and plain `git diff --check` produced no stdout. `wc` reported 534,
768, 275, and 615 lines for the charter, draft, register, and roadmap. The
Python structural check printed `PASS` for the scored rubric, all thirteen
spot-check IDs, all three synthesis families, blocking/non-blocking lists,
coverage omissions, and the release recommendation. These structural checks do
not prove that the source was understood; the direct sampled evidence and
adjudication are recorded in §3.2.

```bash
test -s docs/audit/2026-08-08-worksgood-system/90-independent-review.md
wc -l docs/audit/2026-08-08-worksgood-system/{README.md,40-system-synthesis-draft.md,30-contradiction-and-drift-register.md,31-documentation-sync-plan.md}
python3 - <<'PY'
from pathlib import Path
s = Path('docs/audit/2026-08-08-worksgood-system/90-independent-review.md').read_text()
checks = {
    'scored rubric': '### Overall score' in s and '**Total**' in s,
    'thirteen spot checks': all(f'`S{i}`' in s for i in range(1, 14)),
    'core-runtime family': all(x in s for x in ['Architecture/persistence', 'Lifecycle/completion', 'Model execution/accounting']),
    'trust-safety family': all(x in s for x in ['Agency/evaluation/evolvability', 'Federation identity/custody', 'Review safety/audit', 'Remote execution/Pilot']),
    'product-quality family': all(x in s for x in ['Testing/CI/release', 'Documentation/concepts', 'Operations/config/accounting']),
    'blocking list': all(f'`BR-{i}`' in s for i in range(1, 8)),
    'non-blocking list': '### 6.2 Non-blocking improvements' in s,
    'coverage omissions': '### 5.1 Coverage omissions' in s,
    'release recommendation': '### 6.3 Explicit release recommendation' in s and '**`[RECOMMENDATION]` — HOLD.**' in s,
}
for name, ok in checks.items():
    print(('PASS' if ok else 'FAIL'), name)
raise SystemExit(0 if all(checks.values()) else 1)
PY
git diff --check
```

### 7.4 Limitations

- **`[UNCERTAINTY]`** No full pinned Cargo/smoke execution was performed; sampled runtime statements remain bounded by their upstream command records.
- **`[UNCERTAINTY]`** Static absence searches depend on the named module scope. They do not prove that no indirect or generated path exists outside the search.
- **`[UNCERTAINTY]`** Severity rescoring is a recommendation, not a substitute for product-owner threat/impact decisions.
- **`[FACT]`** This review does not rewrite the draft or resolve product contradictions. It records corrections required for a trustworthy final synthesis.
