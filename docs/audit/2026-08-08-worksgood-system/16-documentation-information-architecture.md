# Documentation information architecture, freshness, duplication, and synchronization

**Audit date:** 2026-08-08

**Evidence checked through:** 2026-08-08

**Audit snapshot:** `b0892ea7496fd2cc8f641417a3d8e33ca9add369`

**Inspection revision:** `98b319c36aa8a21fd4506fc7469fe6d58978cdda` (the only changes from the audit snapshot are the two commits that add the audit charter; `git diff --name-status b0892ea..98b319c` reports only `A docs/audit/2026-08-08-worksgood-system/README.md`)

**Artifact status:** leaf audit; documentation changes are recommended, not performed

**Scope:** repository documentation, documentation-like evidence, generated/help surfaces, and project agent guidance

**Change boundary:** this artifact is the only repository file changed by this task

## 1. Executive abstract

**`[FACT]`** The repository has a large documentation estate with weak global
navigation: 708 tracked Markdown files repository-wide, including 555 under
`docs/` and 56 at the repository root. `docs/` contains 604 files of all types,
about 200,684 Markdown lines, 115 files directly in `docs/`, and two separate
design directories. Only 4 of the 26 immediate `docs/*/` directories have a
README at their root. The nominal canonical index, `docs/KEY_DOCS.md`, mentions
290 of the 555 `docs/**/*.md` paths and was last marked updated on 2026-04-29;
202 Markdown files were added under `docs/` after that date, only two of which
are named in the index. Commands and limitations appear in section 7.

**`[FACT]`** The estate contains distinct kinds of material—entry-point guides,
operator procedures, a conceptual manual, command reference, bundled agent
contracts, ADRs, designs, research, plans, studies, reports, incident evidence,
audits, generated derivatives, website copies, and explicit archives—but most
files carry no machine-readable or uniform declaration of kind, authority,
status, source revision, supersession, or generated source. A bounded scan found
an explicit status marker near the head of only 151 of 555 `docs/**/*.md` files;
254 had either a date marker near the head or a date in the filename. These are
syntax counts, not judgments that every marker is accurate.

**`[VERIFIED]`** A `wg` binary built from this checkout produced 130 parsed
root-command names in `wg --help-all`; a conservative literal comparison found
27 names with no `wg <name>` occurrence in `docs/COMMANDS.md`, despite the
canonical index calling that file the “Complete CLI command reference”
(`docs/KEY_DOCS.md:16`). The same build's `worksgood --help` says bare
`worksgood` on an existing graph does not inspect Pi, plugins, profiles, config,
or services, while the root README says bare `worksgood` verifies Pi and ensures
the plugin without qualifying the existing-graph case (`README.md:93-113`).
These are concrete journey/reference drifts, not proof that every undocumented
internal command should become public.

**`[INFERENCE]`** The central documentation risk is not a shortage of prose. It
is **unbounded authority multiplication**: current user instructions, historical
reports, designs, generated copies, agent guidance, and code-adjacent claims can
all look equally current. Readers must reconstruct chronology and authority from
filenames and context. Confidence: high, because the navigation, status, link,
help, and direct contradiction checks agree. A falsifying check would find a
complete enforced metadata/index/generation system elsewhere in the repository;
the searches in section 7 did not.

**`[FACT]`** There is an important positive control. `AGENTS.md` and `CLAUDE.md`
are byte-identical at this revision, declare themselves layer-2 project guides,
and delegate the universal contract to bundled `src/text/agent_guide.md`
(`AGENTS.md:1-25`; `src/commands/agent_guide.rs:3-15`). Source tests assert
byte parity (`src/commands/agent_guide.rs:132-185`; `tests/integration_init.rs:51-64`),
and a smoke scenario checks fresh-project generation
(`tests/smoke/scenarios/wg_init_writes_lockstep_agent_guides.sh:1-47`). This is
the strongest synchronization pattern observed in the documentation estate.
The tests were inspected, not run in this audit.

**`[RECOMMENDATION]`** The next decision should establish one documentation
manifest and authority model before rewriting narratives: generated reference
from executable definitions; explicit status/snapshot metadata for evidence and
design documents; a generated estate index; CI checks for links, generated
derivatives, agent-guide parity, and root allowlisting; and a staged archive map.
No file should be moved merely because it is old. Section 6 proposes the target
information architecture and acceptance checks.

## 2. Scope and map

### 2.1 Quantitative inventory

**`[FACT]`** Counts below were collected at inspection revision `98b319c`. The
new charter accounts for the one-file increase over the charter's pre-artifact
inventory. “Markdown” means the filename ends in `.md`; “all files” includes
logs, JSON, Typst, patches, code studies, images, and other evidence.

| Surface | Count | Interpretation and limitation |
|---|---:|---|
| Tracked `*.md`, repository-wide | 708 | Includes product docs, agent guidance, terminal-bench material, fixtures, and historical evidence; presence does not establish authority. |
| Documentation-like files found by extension (`md`, `mdx`, `rst`, `txt`) | 735 | Excludes `.git` and `target`; extension-based and therefore incomplete for HTML, Typst, JSON, images, and source-embedded help. |
| `docs/` all files | 604 | 555 `.md`, 16 `.json`, 11 `.typ`, 6 `.rs`, 5 `.log`, 4 `.patch`, 2 `.bib`, and one each of `.txt`, `.ts`, `.png`, `.pdf`, `.mjs`. |
| `docs/**/*.md` | 555 | Approximately 200,684 lines; median 295 lines; 124 exceed 500 lines and 17 exceed 1,000 lines. |
| Root-level tracked `*.md` | 56 | Only `README.md`, `AGENTS.md`, and `CLAUDE.md` are entry/guidance files under the audit classification; see section 2.5. |
| Files directly under `docs/` | 115 | Mixes user reference, ADRs, designs, audits, and reports in one namespace. |
| Immediate `docs/*/` directories | 26 | Only `assets/`, `designs/`, `manual/`, and `research/` have a README at that directory root; `archive/` has a deeper rescued `INDEX.md`, not an archive-root index. |
| Exact duplicate Markdown groups | 3 groups / 6 files | One group is intentional `AGENTS.md`/`CLAUDE.md`; two are terminal-bench result copies. Exact hashing does not detect paraphrased duplicate narratives. |
| Duplicate first-heading groups | 7 | Includes the intentional agent guides, two root prompt-analysis titles, two root documentation-audit titles, and the two compaction-metrics research narratives. |
| Docs with explicit status near head | 151 / 555 | Regex-based; status can still be stale, and unconventional markers are missed. |
| Docs with a date marker near head or dated filename | 254 / 555 | A date is context, not proof of current correctness. |
| `docs/**/*.md` mentioned literally by `KEY_DOCS.md` | 290 / 555 | Literal path coverage only; the file says it is a key-doc index, not necessarily a complete inventory. |
| Local Markdown targets conservatively checked | 341 occurrences in 89 source docs | The simple scanner found 31 unresolved occurrences before excluding absolute machine paths and code-like placeholders; confirmed user-impacting cases are recorded separately. |
| Parsed root commands in checkout-built `wg --help-all` | 130 | Parser reads root listing names; includes internal/advanced commands and omits two unusually spaced completion commands, so this is not a public-surface policy decision. |

**`[FACT]`** Distribution under `docs/` is heavily concentrated in historical or
engineering-evidence categories. Counts here are all files, not only Markdown:

| Subtree/direct area | Files | Index at subtree root | Representative evidence inspected |
|---|---:|---|---|
| direct `docs/` | 115 | `docs/README.md` is a general guide, not an inventory of the 115 files | `README.md`, `KEY_DOCS.md`, `COMMANDS.md`, `AGENT-SERVICE.md`, ADRs |
| `research/` | 131 | yes | `research/README.md:1-35`, organizational patterns, compaction pair, traces |
| `design/` | 82 | no | `design/doc-sync-system.md:1-47`, handler-first design |
| `reports/` | 81 | no | `reports/worker-owned-completion-exit-canary-2026-08-05.md:1-35` |
| `archive/` | 71 | no archive-root index | `archive/2026-04-17-rescued/INDEX.md:1-34` |
| `manual/` | 15 | yes | `manual/README.md:1-42`, Typst and Markdown products |
| `codex-gpt55-investigation/` | 14 | no | `codex-gpt55-investigation/test-results.md:1-26` |
| `pi-integration/` | 14 | no | `pi-integration/integration-plan-v2.md:1-35` |
| `plans/` | 13 | no | `plans/integrated-multi-user-roadmap.md:1-24` |
| `bugs/` | 8 | no | `bugs/tui-pi-chat-launch-enoent.md:1-35` |
| `prod-audit/` | 7 | no | `prod-audit/00-production-readiness-assessment.md:15-29`; follow-up `01` |
| `studies/` | 7 | no | `studies/task-lifecycle-coordinator-deep-survey.md:1-31` |
| `designs/` | 6 | yes | `designs/README.md:1-35` |
| `execution-federation-study/` | 6 | no | `06-decision-memo-and-roadmap.md:1-34` |
| `federation-study/` | 6 | no | `06-decision-memo-and-roadmap.md:1-35` |
| `guides/` | 6 | no | `guides/install.md:1-35` |
| `content-safety-study/` | 4 | no | `04-decision-memo-and-roadmap.md:1-35` |
| `terminal-bench/` | 4 | no | `REFERENCE-terminal-bench-campaign.md:1-35` |
| `audit/` | 3 before this file | no | `audit/doc-sync-apr12-delta-checklist.md:1-30`; current audit charter |
| `agent-reports/` | 3 | no | `agent-reports/tool_call_processing_bug_report.md:1-35` |
| `ops/` | 2 | no | `ops/runbook.md:1-35` |
| `incidents/`, `test-specs/`, `prompts/`, `probes/`, `poetry/` | 1 each | no | incident evidence, test spec, raw self-host prompt, model probe, and Typst poetry artifact |

### 2.2 Document taxonomy

**`[INFERENCE]`** The following taxonomy describes the roles the repository is
already trying to serve. It does not confer authority merely from a path.

| Taxon | Existing examples | What it may establish | What it must not imply |
|---|---|---|---|
| Product entry point | `README.md`, `docs/README.md` | Supported value proposition and first journey, after executable verification | Every historical feature or implementation detail |
| Getting-started guide | `docs/quickstart-pi-openrouter.md`, `docs/guides/install.md` | A bounded, supported human journey | General product architecture or provider availability forever |
| Conceptual manual | `docs/manual/*.typ`, `wg-manual.typ` | Stable concepts and vocabulary | Exact CLI flags or current runtime behavior unless generated/checked |
| CLI/config/reference | `docs/COMMANDS.md`, `docs/models.md`, `docs/config-*.md`, source help | Exact supported syntax when generated from executable definitions | Roadmap intent or historical aliases as defaults |
| Agent contract | `src/text/agent_guide.md`, `AGENTS.md`, `CLAUDE.md`, `docs/AGENT-GUIDE.md` | Normative role behavior at its declared layer | Product behavior outside that contract |
| Operator guide/runbook | `docs/ops/`, `docs/AGENT-SERVICE.md`, `docs/LOGGING.md` | Current deployment and recovery procedure, if versioned and exercised | Security/correctness certification |
| ADR/decision | `docs/ADR-*.md`, terminal study decision memos | Accepted intent, invariants, tradeoffs, rejected alternatives | Proof that enforcement landed or remains wired |
| Design/specification | `docs/design/`, `docs/designs/`, root `*design*.md`, `docs/test-specs/` | Proposed/implemented shape at a named revision | Current behavior without implementation and test evidence |
| Research/study/plan | `docs/research/`, `docs/studies/`, `docs/*-study/`, `docs/plans/` | Investigation context and proposed work | Shipped capability; a directory-level “mostly implemented” label is insufficient |
| Report/audit/incident/bug | `docs/reports/`, `docs/prod-audit/`, `docs/incidents/`, `docs/bugs/`, root reports | Point-in-time observations and evidence | Present state after subsequent fixes unless revalidated |
| Generated derivative | manual Markdown, website HTML, embedded/bundled text, captured help | Rendered access format tied to a declared source and generation digest | Independent authority or freshness |
| Archive | `docs/archive/` | Provenance and historical context | Current instructions unless explicitly revalidated |

### 2.3 Authority and freshness matrix

**`[RECOMMENDATION]`** This is the proposed authority/freshness model. “Higher”
means preferred for the stated question, not universally more valuable.

| Question / class | Primary authority | Corroboration | Observed state | Required freshness control |
|---|---|---|---|---|
| What command/flag exists? | Current CLI definition and help from the release binary (`src/cli.rs` plus dispatch) | Generated command reference and CLI tests | Source help is live; `COMMANDS.md` is manually maintained and incomplete by literal audit | Generate reference from the same command schema; CI diff against committed output |
| What does a command do? | Executed behavior plus current implementation | Integration/smoke/human-flow tests | Documentation often cites tests/reports, but claims and current code are mixed | Supported journey matrix with last-executed revision/environment |
| What should the architecture guarantee? | Accepted ADR/decision with explicit status | Enforcement sites and negative tests | ADRs and decision memos exist, but studies may still say “draft” after code ships | ADR index with accepted/superseded status; implementation links do not auto-close decisions |
| What should a worker/chat/dispatcher do? | `src/text/agent_guide.md` bundled by `src/commands/agent_guide.rs` | Project layer in lock-step `AGENTS.md`/`CLAUDE.md` | Strong source/test synchronization; runtime invocation was blocked in this worker context | Preserve source inclusion, parity tests, and smoke scenario; version the contract |
| How is this repository developed? | Byte-identical root `AGENTS.md`/`CLAUDE.md` | Build/CI configuration | Current and test-guarded, but 761 lines duplicate broad product narratives | Keep project-only content; link to authoritative references rather than restating changing subsystems |
| What does an operator do now? | Versioned runbook for a declared release/profile | Live/scripted operator flow | `ops/` has two files and no index; older audit verdicts sit nearby without supersession routing | Release applicability, prerequisites, last rehearsal, rollback, and owner metadata |
| What did an audit/report observe? | The report at its pinned revision | Follow-up/closure record | Good individual examples include revision/date, but directories lack bundle indexes and supersession | Immutable evidence plus `superseded_by`/`closed_by`; never rewrite original verdict silently |
| Which manual file is authoritative? | **Unresolved today** | `manual/README` says unified Typst; sync script says Typst generally and converts chapter Typst | Conflicting source-of-truth declarations; generated Markdown has no CI staleness gate found | Choose one source graph; record generator version/digest; CI regenerate-and-diff |
| What docs exist? | Generated estate manifest | Curated audience landing pages | `KEY_DOCS.md` is aging and covers about half of docs Markdown | Generate inventory; curate only support level and reading paths |
| Is a historical file still applicable? | Explicit per-file status and supersession | Code/test spot-check | Mostly inferred from names/dates | Default undated reports/designs to “status unknown,” not current |

**`[FACT]`** Freshness states from the audit charter apply here: snapshot-current,
dated-recent, dated-aging, dated-stale, undated, historical/archived,
superseded, and future/proposed. `KEY_DOCS.md` is dated-aging at the audit date
(`docs/KEY_DOCS.md:1-5`). The root README and `docs/README.md` are undated even
though they make rapidly changing setup claims. The explicit archive is
historical by path and its rescued index records provenance
(`docs/archive/2026-04-17-rescued/INDEX.md:1-16`).

### 2.4 Current information flows and duplication points

```text
src/cli.rs + command implementations ──> runtime `wg --help[--all]`
             ╲                         (no COMMANDS generator found)
              ╲── manually restated ──> docs/COMMANDS.md / GUIDE / README

src/text/agent_guide.md ─include_str──> `wg agent-guide`
          project template ───────────> AGENTS.md == CLAUDE.md
          educational restatement ────> docs/AGENT-GUIDE.md

manual chapter *.typ ─sync-docs.sh──> chapter *.md ─cat──> wg-manual.md
wg-manual.typ ─typst compile─────────> untracked/uncommitted PDF output
chapter/unified authority declaration: conflicting

quickstart Markdown ─manual copy?────> website/quickstart-pi-openrouter.html
(no repository generator or CI edge found)

ADRs/designs/studies/plans ─manual interpretation──> README/guides/agent guidance
reports/audits/follow-ups ─no global bundle index──> reader reconstructs chronology
```

**`[FACT]`** `scripts/sync-docs.sh:1-8` says Typst is source of truth;
`104-118` converts five chapter Typst files, concatenates chapter Markdown into
`wg-manual.md`, and separately converts organizational patterns. It does not
generate `docs/COMMANDS.md`, `KEY_DOCS.md`, root guidance, or the quickstart
HTML. Searches of `Makefile`, `.github/workflows/`, `tests/`, and `scripts/`
found no Markdown link check or regenerate-and-diff gate for these surfaces.

### 2.5 Root-clutter analysis

**`[FACT]`** The 56 root Markdown files classify by filename/content role as:
3 entry/guidance files (`README.md`, `AGENTS.md`, `CLAUDE.md`), 28
report/audit/investigation files, 15 design/plan/migration files, and 10 other
project notes or implementation summaries. This classification is an audit
heuristic, not a requested move.

**`[FACT]`** Of the 53 non-entry root Markdown files, 48 have no inbound literal
filename reference from `README.md`, `AGENTS.md`, `CLAUDE.md`, or any Markdown
under `docs/`. The five with at least one such reference are `context.md`,
`hardening-integration-summary.md`, `action-plan-2026-04-14.md`, and the two
2026-06-03 autopoietic-loop reports. Zero inbound references do not prove zero
value, but they do show the root is not functioning as an intentional index.

**`[FACT]`** Root clutter also carries link debt: `action-plan-2026-04-14.md`
links to root `issues-2026-04-14.md` and `damage-diff-2026-04-14.patch`, but
those files now live in the rescued archive under a different path. Two root
documentation audits remain side by side with nearly identical H1s
(`root-documentation-audit-2026-04-12.md` and
`root-level-documentation-audit-findings-2026-04-12.md`). The current root count
is nearly unchanged from the prior audit's stated 57 files
(`root-level-documentation-audit-findings-2026-04-12.md:8-20`).

**`[INFERENCE]`** Root placement is currently an accidental status signal:
readers may infer that a root report is more current than an explicitly indexed
`docs/` document, even though almost all root notes are neither linked nor
status-marked. Confidence: high. The appropriate remedy is an inventory and
redirect/archive plan, not immediate bulk movement.

## 3. Findings

### DOC-001 — the canonical index is curated, aging, and not an estate index

**`[FACT]`** **State:** partial. **Severity:** S2. **Likelihood:** observed.
**Confidence:** high. `docs/KEY_DOCS.md:1-5` calls itself the canonical list of
key docs and says it was last updated 2026-04-29. It mentions 290 of 555 docs
Markdown files. Git history reports 202 Markdown files added under `docs/` after
2026-04-29 and only two are named literally in `KEY_DOCS.md`. The index still
lists a nonexistent `docs/design-cyclic-wg.md` at line 339; four other missing
path tokens are struck-through removed entries and therefore are not broken
reader promises.

**`[INFERENCE]`** “Key docs” and “all docs” are different products, but the file
currently performs both discovery and status classification. That ambiguity
makes omissions look accidental and stale status labels look authoritative.
Affected boundary: every reader selecting evidence or guidance. Owner:
documentation architecture. Linked recommendations: DOC-REC-001, 002.

### DOC-002 — first-run/setup narratives conflict across primary user surfaces

**`[CONTRADICTION]`** **State:** current text conflict; runtime scope partly
verified. **Severity:** S2. **Likelihood:** observed for readers. **Confidence:**
high. `README.md:101` says bare `worksgood` verifies Pi and ensures the plugin.
The checkout-built `worksgood --help` says an existing-graph bare launch opens a
setup-neutral TUI and “does not inspect Pi, plugins, profiles, concierge state,
config, or services”; it limits bootstrap work to a new repository. The README
sentence does not state that distinction.

**`[CONTRADICTION]`** `docs/README.md:152-184` directs first-time users to a
Claude-default executor wizard, old `executor` keys, and `opus`/`sonnet`/`haiku`
models. Current project guidance and routing source describe handler-first model
specs, explicit route selection, and deprecated executor keys
(`AGENTS.md:82-105`; `src/dispatch/handler_for_model.rs:26-59`). The root README
instead calls Pi the “sole model plane” (`README.md:115-165`). These may describe
attended versus unattended audiences, but the docs landing page does not make
that scope distinction.

### DOC-003 — document status and applicability are usually implicit

**`[FACT]`** **State:** partial. **Severity:** S2. **Likelihood:** likely.
**Confidence:** high for metadata absence, medium for reader impact. Only 151 of
555 docs Markdown files matched an explicit status marker near the head. The
`research/README.md:3-22` says most documents describe already implemented
behavior while some are exploratory snapshots; it supplies no per-file map.
`designs/README.md:3-22` says behaviors in that directory are already
implemented and historical/didactic, but the much larger singular `design/`
directory has no index and contains mixed Proposed, Design, Reference, and
implemented material.

**`[FACT]`** The 2026-04-29 doc-sync report already recorded the underlying debt:
92 design files lacking contributor headers, about 17 stale design status
headers, unresolved `design/` versus `designs/`, and research documents needing
resolved-by/date/task markers (`docs/doc-sync-audit-2026-04-29.md:199-205`). The
current inventory shows the structural debt persisted while the estate grew.
The three `agent-reports/` files similarly make present-tense issue claims; for
example `tool_call_processing_bug_report.md:1-35` and
`ui_freeze_bug_report.md:1-48` provide symptoms and suggestions but no date,
revision, command output, implementation citation, or closure link. Their claims
remain unverified reports, not current product facts.

### DOC-004 — generated and copied documentation lacks a complete enforced source graph

**`[CONTRADICTION]`** **State:** partial. **Severity:** S2. **Likelihood:**
possible drift, observed authority ambiguity. **Confidence:** high.
`docs/manual/README.md:30-42` says `wg-manual.typ` is the authoritative unified
manual; `scripts/sync-docs.sh:1-8` says Typst generally is source of truth, then
converts the five chapter Typst files and concatenates their Markdown outputs
rather than converting the unified file (`scripts/sync-docs.sh:102-118`). The
script's last-resort path can copy raw Typst into a `.md` file
(`scripts/sync-docs.sh:83-99`). No CI regenerate-and-diff check was found.

**`[FACT]`** Root README says the Pi quickstart also ships as
`website/quickstart-pi-openrouter.html` (`README.md:119-126`). The Markdown was
last changed at commit `4019645` on 2026-07-31, while the HTML was last changed
at `b9f0676` on 2026-07-27. No generator edge was found in `scripts/`,
`Makefile`, or workflows. Different formats prevent a meaningful raw diff, so
content drift remains an uncertainty rather than a declared contradiction.

### DOC-005 — the “complete” command reference does not cover the full current help surface

**`[VERIFIED]`** **State:** partial. **Severity:** S2. **Likelihood:** observed.
**Confidence:** high for literal absence, medium for intended public coverage.
The checkout-built `target/debug/wg --help-all` exited 0 and exposed 130 parsed
root command names. Twenty-seven had no literal `wg <command>` occurrence in
`docs/COMMANDS.md`: `agent-guide`, `land`, `spawn-task`, `candidate`, `contract`,
`coordinator`, `dev-check`, `disk`, `doctor`, `fed-node`, `finalize`, `identity`,
`incomplete`, `nex`, `pi-handler`, `pi-plugin`, `pi-watchdog`, `pilot`,
`provider`, `reset`, `review`, `session`, `submit`, `tui-nex`, `tui-pty`,
`upgrade`, and `worktree`. `docs/KEY_DOCS.md:16` calls the reference complete.
Some names are internal or advanced; the deficiency is also the absence of a
published/public classification in the help-to-reference pipeline.

### DOC-006 — root clutter and duplicate narratives obscure entry points

**`[FACT]`** **State:** current. **Severity:** S3. **Likelihood:** observed.
**Confidence:** high. Section 2.5 records 53 non-entry root Markdown files, 48
without inbound filename references from living documentation. The root includes
multiple prompt analyses, coordinator lifecycle designs/audits, verify-timeout
research/design/implementation/migration documents, and two root-doc audits.
The root README links none of these.

**`[FACT]`** Exact hashes find little duplication because most duplication is
narrative rather than byte-identical. Duplicate-H1 and prior-audit evidence
identify the root audit pair and the
`research/compaction-metrics-and-visibility.md` /
`research/compaction-metrics-visibility.md` pair; the 2026-04-29 audit explicitly
called the latter duplicate (`docs/doc-sync-audit-2026-04-29.md:199-205`).
Manual chapter pairs, assembled manual pairs, quickstart HTML, and agent-guide
layers are intentional format/layer duplication that need synchronization rather
than deletion.

### DOC-007 — link integrity is not gated

**`[FACT]`** **State:** current. **Severity:** S2 for the primary README asset,
S3 generally. **Likelihood:** observed. **Confidence:** high for confirmed
paths. `README.md:9` embeds `docs/assets/wg-tui.gif`, but `docs/assets/` contains
only `README.md`; the primary landing page's hero image is broken in this
checkout. This is known placeholder debt rather than an accidental deletion:
`docs/assets/README.md:1-12` says the GIF is referenced “until the real capture
lands” so renderers show alt text. `docs/plans/integrated-multi-user-roadmap.md:5-9`
links to removed `federation-and-distributed-sync.md`.
`docs/design-cyclic-workgraph.md` links to missing
`dag-assumptions-survey.md`. Historical/root links add further debt.

**`[UNCERTAINTY]`** The simple Markdown scanner also reported absolute local
machine paths encoded as links in `docs/design-wg-login-openrouter.md`, a
placeholder link target `path`, and other code-like forms. Those are not counted
as confirmed reader-navigation defects without a Markdown-aware policy. A real
link checker needs explicit allow/ignore rules for evidence citations, absolute
historical paths, anchors, generated assets, and archived material.

### DOC-008 — agent-guide parity is an effective synchronization control

**`[FACT]`** **State:** shipped/current. **Severity:** S4 positive control.
**Confidence:** high. `AGENTS.md` and `CLAUDE.md` have the same SHA-256
`90083d8e...ec723` and 761 lines each. Their managed header delegates universal
behavior to `wg agent-guide` (`AGENTS.md:1-25`). The bundled source is included
at compile time (`src/commands/agent_guide.rs:3-15`). Unit/integration and smoke
source assert parity and fresh-init behavior (`src/commands/agent_guide.rs:132-185`;
`tests/integration_init.rs:51-64`;
`tests/smoke/scenarios/wg_init_writes_lockstep_agent_guides.sh:27-47`).

**`[UNCERTAINTY]`** Running `cargo run --bin wg -- agent-guide` inside this worker
context was refused by the worker-control authority boundary, so this audit did
not verify emitted text at runtime. Source inclusion and inspected tests support
structure, not this environment's command reachability.

### DOC-009 — point-in-time reports lack bundle-level supersession navigation

**`[FACT]`** **State:** partial. **Severity:** S2. **Likelihood:** likely.
**Confidence:** high. `docs/prod-audit/audit-fed.md:34-40` says the federation
compat handshake logic is unwired. Current source implements an HTTP-store
handshake that fetches `/version` and calls `check_compat`
(`src/identity/transport.rs:163-169`, `583-607`). The next-day follow-up updates
the overall production verdict (`docs/prod-audit/01-production-readiness-followup.md:1-34`),
but `docs/prod-audit/` has no README or manifest directing readers from the
initial component finding to closure.

**`[INFERENCE]`** The earlier report is valuable and should remain immutable; the
problem is discoverability of its applicability, not that historical evidence
contains an old result. The same pattern applies to bug/fix reports, studies
whose sparks later shipped, and plan v1/v2 sequences. Linked recommendation:
DOC-REC-006.

### DOC-010 — version and naming narratives drift unless sourced from constants

**`[CONTRADICTION]`** **State:** current doc drift. **Severity:** S2.
**Likelihood:** observed. **Confidence:** high. `docs/AGENT-SERVICE.md:278-299`
says the handler is derived from a provider prefix and presents bare
`openrouter:`, `ollama:`, and `vllm:` as normal examples. Current routing source
says the leading token is always a handler and labels those leading provider
forms deprecated (`src/dispatch/handler_for_model.rs:26-59`); strict parsing
warns and rewrites during the current release
(`src/config.rs:2655-2666`, `2845-2874`). Root agent guidance uses the newer
handler-first naming (`AGENTS.md:82-97`).

**`[FACT]`** Compatibility constants are at least centralized in code:
`WG_FED_COMPAT_VERSION = "0.4.0"` (`src/identity/mod.rs:118-141`),
`WG_EXEC_COMPAT_VERSION = "0.1.0"` (`src/providers/mod.rs:52-62`), and
`WG_PI_PLUGIN_COMPAT_VERSION = "0.2.0"` (`src/pi_plugin/mod.rs:25-45`). Some
recent designs cite these exact values, while older reports correctly preserve
older values. Literal version references should therefore be generated or
marked “observed at revision,” not globally updated in historical evidence.

## 4. Contradictions and drift

**`[FACT]`** This representative table preserves both sides and does not treat
age or majority wording as adjudication.

| ID | Claim A | Claim B / evidence | Authority and scope | Severity / confidence | Resolution state |
|---|---|---|---|---|---|
| `DOC-DRIFT-001` | Bare `worksgood` verifies Pi and ensures the plugin (`README.md:93-113`). | Checkout-built `worksgood --help` says existing-graph bare launch inspects none of Pi/plugins/profiles/config/services; new repositories get route-free bootstrap. | Executed help and dispatch source outrank unqualified journey prose for this revision. | S2 / high | **Open:** qualify existing vs new repository. |
| `DOC-DRIFT-002` | First setup defaults to Claude executor and `opus` aliases with `[coordinator].executor` (`docs/README.md:152-184`). | Root current journey is attended Pi or explicit graph-only; current routing deprecates executor keys and requires explicit routes (`README.md:93-165`; `AGENTS.md:82-105`; source parser). | Docs landing page is stale for current onboarding; non-Pi handlers still exist for advanced automation. | S2 / high | **Open:** split attended, graph-only, and unattended automation journeys. |
| `DOC-DRIFT-003` | Provider prefix selects handler; bare `openrouter:` is a normal example (`docs/AGENT-SERVICE.md:278-299`). | Leading token is a handler; bare provider prefix warns/rewrites (`src/dispatch/handler_for_model.rs:26-59`; `src/config.rs:2655-2874`). | Current E2 source and project guidance are authoritative for parser behavior. | S2 / high | **Open:** synchronize service guide; retain migration note. |
| `DOC-DRIFT-004` | Unified `wg-manual.typ` is authoritative (`docs/manual/README.md:30-42`). | Sync script calls Typst source of truth but generates Markdown from chapter Typst and concatenates chapter Markdown (`scripts/sync-docs.sh:1-8`, `102-118`). | Authority among Typst sources is unresolved; generated Markdown is derivative. | S2 / high | **Open:** choose and encode one source graph. |
| `DOC-DRIFT-005` | `KEY_DOCS.md` is canonical and audit-verified as of 2026-04-29 (`:1-5`). | It mentions 290/555 docs Markdown paths; 202 Markdown files were added after its date, only two mentioned; line 339 names missing `docs/design-cyclic-wg.md`. | Curated reading list remains useful but is not a current inventory. | S3 / high | **Open:** separate generated inventory from curated key paths. |
| `DOC-DRIFT-006` | `COMMANDS.md` is a complete reference (`docs/KEY_DOCS.md:16`). | 27 of 130 parsed root help names have no literal `wg <name>` entry. | Some commands may intentionally be internal; no public/internal declaration bridges the two. | S2 / medium-high | **Open:** publish classification and generate public reference. |
| `DOC-DRIFT-007` | Federation handshake is unwired (`docs/prod-audit/audit-fed.md:39-40`). | Current `HttpStore::handshake` calls `check_compat` (`src/identity/transport.rs:583-607`); a follow-up changes the verdict. | Historical report is correct only at its pinned state; current implementation supersedes behavior claim. | S3 / high | **Resolved historically, navigation open:** add `superseded_by` bundle index. |
| `DOC-DRIFT-008` | Content-safety terminal memo remains “draft for evaluation” (`docs/content-safety-study/04-decision-memo-and-roadmap.md:14-16`). | ADRs, `src/review/`, CLI commands, smoke manifests, and current project guidance describe shipped review slices. | Memo remains valuable decision history; status is stale unless scoped to study wave. | S3 / high | **Open:** mark accepted/superseded-by ADRs and implementation milestone. |
| `DOC-DRIFT-009` | Root README embeds current TUI hero image (`README.md:9`). | `docs/assets/wg-tui.gif` does not exist; `docs/assets/` contains only its README. | Primary landing-page path is directly checkable. | S2 / high | **Open:** restore/generate asset or remove reference. |
| `DOC-DRIFT-010` | Website quickstart is the same styled path (`README.md:119-126`). | Markdown changed four days after HTML; no generation edge found. | Content equivalence was not fully normalized/diffed. | S3 / medium | **Uncertain:** establish generator/digest before calling identical. |

**`[FACT]`** Apparent contradiction resolved during checking: `AGENTS.md` and
`CLAUDE.md` duplicate 761 lines, but this duplication is intentional and
byte-parity is tested. It should not be deduplicated by replacing one with a
symlink without first preserving tool compatibility and the existing tests.

## 5. Risks and gaps

| ID | Label | Severity | Risk/gap | Boundary and uncertainty |
|---|---|---:|---|---|
| `DOC-RISK-001` | `[INFERENCE]` | S1 | A stale safety, setup, or operations narrative can authorize the wrong human action even when code is safe. | Highest at install/config/secrets/federation/review/exec journeys. This audit did not execute destructive or credentialed flows. |
| `DOC-RISK-002` | `[FACT]`; `[INFERENCE]` | S2 | Half-indexed estate plus weak status metadata makes search ranking/path placement substitute for authority. | Affects users, agents, maintainers, and later auditors. |
| `DOC-RISK-003` | `[INFERENCE]` | S2 | Manual reference maintenance cannot reliably track 130+ root commands and many subcommands. | Literal absence is measured; public/internal policy remains undecided. |
| `DOC-RISK-004` | `[FACT]` | S2 | No repository-wide link or asset gate was found; the root hero asset is already missing. | The ad-hoc scanner has false positives and is not proposed unchanged. |
| `DOC-RISK-005` | `[INFERENCE]` | S2 | Reports without supersession routing can reverse current conclusions—especially security/readiness findings. | Preserve immutable reports; add closure metadata rather than rewriting history. |
| `DOC-RISK-006` | `[FACT]`; `[UNCERTAINTY]` | S2 | Manual and website derivatives have incomplete source/generation controls. | Raw format differences prevent declaring all copies stale; no CI gate was found. |
| `DOC-RISK-007` | `[INFERENCE]` | S3 | Root clutter and singular/plural design trees increase accidental duplication and weaken ownership. | No movement or deletion was attempted; Git history/URLs may constrain later changes. |
| `DOC-RISK-008` | `[UNCERTAINTY]` | S2 | This audit did not semantically validate all 200k documentation lines or every ADR claim against code/tests. | Samples cover every major subtree; leaf system audits own deep domain verification. |
| `DOC-GAP-001` | `[FACT]` | S3 | There is no declared support-level matrix for public, advanced, internal, migration-only, experimental, and historical commands/features. | Prevents clean CLI-reference adjudication. |
| `DOC-GAP-002` | `[FACT]` | S3 | There is no archive-root index or global redirect/supersession manifest. | The rescued archive has good local provenance; other history is harder to traverse. |
| `DOC-GAP-003` | `[UNCERTAINTY]` | S3 | Website deployment and external theory/mission pages were not fetched or browser-tested. | Local website copies were inventoried only. |

## 6. Recommendations

### 6.1 Factual synchronization work

1. **`DOC-REC-001` — `[RECOMMENDATION]` (P0, documentation tooling): create a generated estate manifest.** Each tracked documentation artifact should record path, taxon, audience, authority class, status, owner, `valid_as_of` revision/date, `supersedes`/`superseded_by`, source-of-truth, generated outputs, and evidence/test links. Curated landing pages select from this manifest; they do not double as inventory. **Acceptance:** all 604 `docs/` files and 56 root Markdown files are classified or explicitly ignored; `KEY_DOCS` omissions cannot be silent.
2. **`DOC-REC-002` — `[RECOMMENDATION]` (P0, product docs): reconcile the three supported first journeys.** Keep distinct flows for (a) attended existing graph, (b) new route-free graph, and (c) unattended automation. Synchronize `README.md`, `docs/README.md`, quickstart, `worksgood --help`, and relevant tests. **Acceptance:** an executable journey table shows command, mutation, credential/plugin/profile/service effects, source handler, and last human-flow test.
3. **`DOC-REC-003` — `[RECOMMENDATION]` (P0, release/CLI): generate the public command reference from the command schema.** Add an explicit `public`, `advanced`, `internal`, `migration`, or `hidden` support tag; generate command signatures/help and preserve authored examples in keyed include blocks. **Acceptance:** checkout-built `wg --help-all` and committed reference differ only for intentionally excluded, manifest-tagged commands.
4. **`DOC-REC-004` — `[RECOMMENDATION]` (P0, CI/docs): add a policy-aware link/asset checker.** Start with user-facing/current docs; handle anchors, absolute historical evidence paths, and archives by policy. **Acceptance:** root README assets and current local links exist; archived broken links are either repaired, recorded, or exempted with reason.
5. **`DOC-REC-005` — `[RECOMMENDATION]` (P1, manual/website): encode generation DAGs.** Decide whether unified or chapter Typst is authoritative; generate chapter Markdown, assembled Markdown/PDF, organizational patterns, and website quickstart deterministically. **Acceptance:** a clean checkout regeneration produces no diff; generated files contain source revision and generator version; CI runs the check.
6. **`DOC-REC-006` — `[RECOMMENDATION]` (P1, evidence owners): add bundle indexes and supersession edges.** Index `prod-audit`, studies, investigations, bug/fix chains, incidents, and reports with immutable observation revision plus latest disposition. **Acceptance:** readers entering any report bundle can reach the current closure/deferred state without searching Git history.
7. **`DOC-REC-007` — `[RECOMMENDATION]` (P1, WG maintainers): preserve the agent-guide positive control and reduce restatement.** Keep `src/text/agent_guide.md` bundled, maintain byte parity for root project guides, and replace volatile subsystem narratives in the 761-line layer-2 guides with links/generated excerpts where safe. **Acceptance:** existing parity unit/integration/smoke checks remain, and current model/version facts have one declared source.

### 6.2 Target information architecture

**`[RECOMMENDATION]`** The target below is conceptual; **do not move files as
part of this audit**. First publish a path-by-path mapping, redirects, link
impact, owners, and archive decisions.

```text
README.md                         # product promise + three supported journeys
AGENTS.md == CLAUDE.md            # project-only layer 2, parity tested

docs/
  README.md                       # generated/curated audience router
  manifest.toml                   # complete machine-readable estate inventory
  getting-started/
    attended.md
    graph-only.md
    unattended-automation.md
    install.md
  concepts/
    manual.typ                    # one declared conceptual source graph
    generated/                    # md/pdf, regenerate-and-diff
  reference/
    cli.md                        # generated signatures + keyed authored examples
    config.md
    storage-and-formats.md
    compatibility.md              # generated current constants + protocol links
  agents/
    project-guidance.md
    service-and-dispatch.md
  operations/
    README.md
    runbooks/
    troubleshooting/
    security/
  architecture/
    README.md                     # status index
    adr/
    designs/
  contributor/
    development.md
    testing.md
    worktrees.md
  evidence/
    audits/<date-or-release>/
    reports/<topic>/
    incidents/
    bugs/
    test-specs/
    benchmarks/
  research/
    README.md
    studies/
    plans/
  archive/
    README.md                     # provenance, policy, redirects
    <year>/<topic>/
```

**`[RECOMMENDATION]`** Authority should flow downward from executable/source
truth to generated reference, and from accepted decisions to explicitly linked
implementation—not laterally by copying prose. Evidence flows in the other
direction: immutable reports preserve what was observed, while indexes point to
closure. Website pages should be build outputs or consumers of the same source,
never another hand-maintained authority.

### 6.3 Archival and root-cleanup strategy

8. **`DOC-REC-008` — `[RECOMMENDATION]` (P1, documentation owners): approve a root allowlist and staged mapping.** Proposed allowlist: product README, license/governance files, and tool-required agent guidance. Classify the other 53 root Markdown files individually as current guide, evidence bundle, design/decision, archive, duplicate, or delete-candidate. **Acceptance:** every proposed move has inbound-link and Git-history review; redirects/link updates land in the same change; no bulk move without owner review.
9. **`DOC-REC-009` — `[RECOMMENDATION]` (P1, evidence governance): archive by applicability, not age.** A dated report may remain current evidence; an undated design may already be superseded. Archive only after naming the replacement or declaring no current replacement. **Acceptance:** archived artifacts retain original revision/date, stable link/redirect where feasible, and a reason/disposition.
10. **`DOC-REC-010` — `[RECOMMENDATION]` (P2, design/research owners): merge the `design/` and `designs/` taxonomy only after status inventory.** One future directory should contain accepted/proposed designs with metadata; historical research remains separate. **Acceptance:** all 88 current design files have status and successor mapping before physical consolidation.

### 6.4 Product/code decisions discovered but not implemented here

11. **`DOC-REC-011` — `[RECOMMENDATION]` (P0, product owner): decide whether Pi is the sole attended plane, sole recommended plane, or sole plane overall.** Current code retains multiple handlers, while root marketing uses “sole model plane.” **Acceptance:** one scoped sentence is reused across entry point, docs landing, setup help, and architecture.
12. **`DOC-REC-012` — `[RECOMMENDATION]` (P1, CLI/product owner): decide which of the 27 reference-absent commands are public.** Internal commands should be marked/hidden; public commands should be generated into reference. **Acceptance:** no accidental public surface remains undocumented.
13. **`DOC-REC-013` — `[RECOMMENDATION]` (P1, audit roadmap): create follow-up implementation tasks only after manifest review.** Separate factual text fixes (broken asset, stale examples), tooling (generators/checkers), IA moves, and product semantics. **Acceptance:** each task names source authority, files, executable validation, redirect/archive policy, and rollback.

## 7. Evidence appendix

### 7.1 Environment and execution status

**`[VERIFIED]`** Commands were run on 2026-08-08 from
`/home/bot/wg/.wg-worktrees/agent-10` on Linux `6.8.0-90-generic x86_64`, Rust
`1.96.0`, Cargo `1.96.0`, and Python `3.12.3`. Static inspection revision was
`98b319c36aa8a21fd4506fc7469fe6d58978cdda`; exit status was 0 unless noted.

```bash
git diff --name-status b0892ea7496fd2cc8f641417a3d8e33ca9add369..HEAD
find docs -type f | wc -l
find docs -type f -name '*.md' | wc -l
git ls-files '*.md' | wc -l
git ls-files '*.md' | awk 'index($0,"/")==0' | wc -l
cmp -s AGENTS.md CLAUDE.md
find docs -type f -printf '%f\n' | awk '...'     # extension histogram
```

Bounded results: only the charter was added after the pinned product snapshot;
604 docs files, 555 docs Markdown files, 708 tracked Markdown files, 56 root
Markdown files, and byte-identical root agent guides.

**`[VERIFIED]`** Help/reference commands:

```bash
cargo run --locked --quiet --bin wg -- --help
cargo run --locked --quiet --bin worksgood -- --help
target/debug/wg --help-all > /tmp/audit-wg-help-all.txt
```

All three exited 0. `wg --help` reported “... and 118 more (`--help-all`)”;
`worksgood --help` printed the existing/new repository product boundary; the
root-command parser extracted 130 names from `--help-all`. Compilation emitted
warnings but completed. `cargo run --locked --quiet --bin wg -- agent-guide`
exited 1 with `worker_control.operation_refused`; therefore emitted agent-guide
behavior is not marked verified.

### 7.2 Inventory/search methods

**`[VERIFIED]`** A Python inventory grouped all `docs/` files by immediate and
second-level path; another counted Markdown lines, near-head status/date markers,
and largest files. The status regex recognized `Status`, `Document status`,
`Implementation status`, and `Artifact status`; nonstandard metadata may be
missed. The largest Markdown surfaces included `docs/COMMANDS.md` (4,417 counted
lines including final-line convention), `docs/test-specs/trace-replay-test-spec.md`
(1,845), `docs/manual/wg-manual.md` (1,505), and `docs/AGENT-GUIDE.md` (1,261).

**`[VERIFIED]`** Exact duplicate detection used SHA-256 over 708 Markdown files.
Duplicate-H1 detection lowercased the first `#` heading. These methods do not
detect paraphrase, excerpt reuse, or semantically equivalent generated formats.

**`[VERIFIED]`** Index coverage tested whether each current docs path appeared
literally in `docs/KEY_DOCS.md`. Post-index growth used:

```bash
git log --since='2026-04-30' --diff-filter=A --name-only --format='' -- docs \
  | sort -u
```

Result: 229 docs paths of all types added, 202 Markdown; two appeared literally
in `KEY_DOCS.md`. This is commit-history context, not proof that every added file
belongs in a curated key-doc list.

**`[VERIFIED]`** The local-link scanner parsed inline Markdown `[]()` targets,
resolved relative paths, and ignored URL/mail/anchor/data schemes. It checked 341
occurrences in 89 source documents and initially reported 31 unresolved
occurrences across 10 documents. Absolute workstation paths, placeholder/code
links, and archived references create false positives; only manually confirmed
cases are findings.

**`[VERIFIED]`** Root inbound-reference analysis searched `README.md`, both agent
guides, and `docs/**/*.md` for each of the 53 other root filenames. It found 48
with zero literal inbound references. This does not inspect non-Markdown links,
external links, Git history, or conceptual references that omit filenames.

### 7.3 Primary evidence and subtree samples

**`[FACT]`** Major primary evidence inspected:

| Evidence | Observation | Class/freshness |
|---|---|---|
| `README.md:1-9`, `91-165`, `176-225` | product entry, broken hero asset path, setup/Pi claims, documentation links | E4, undated |
| `docs/README.md:1-184` | docs landing, concepts, old first-time setup | E4, undated |
| `docs/KEY_DOCS.md:1-25`, `339`, `360-376` | canonical claim/date, missing path, audit/archive lists | E4, dated-aging |
| `docs/COMMANDS.md` | 4,416 physical newline count; manual reference searched against help | E4, undated |
| `src/cli.rs:38-2720` and checkout help | command definitions and rendered root help | E2/E1, snapshot-current |
| `src/dispatch/handler_for_model.rs:26-59`; `src/config.rs:2655-2874` | handler-first rule and current warn/rewrite phase | E2, snapshot-current |
| `docs/AGENT-SERVICE.md:278-299` | older provider-first examples | E4, undated |
| `src/text/agent_guide.md`; `src/commands/agent_guide.rs:3-15,132-185` | bundled universal contract and parity tests | E2/E3, snapshot-current; runtime command blocked |
| `AGENTS.md:1-25,82-105`; `CLAUDE.md` | layer-2 contract and current routing narrative | E4/E2 template output, snapshot-current |
| `tests/integration_init.rs:51-64`; smoke init scenario | parity assertions | E3, inspected not run |
| `docs/manual/README.md:1-42`; `scripts/sync-docs.sh:1-8,83-118` | manual authority/generation conflict | E4/E2, snapshot-current |
| `docs/prod-audit/audit-fed.md:34-40`; follow-up `01:1-34`; `src/identity/transport.rs:163-169,583-607` | historical handshake finding, closure/current implementation | E4/E2, point-in-time vs snapshot-current |
| `src/identity/mod.rs:118-141`; `src/providers/mod.rs:52-62`; `src/pi_plugin/mod.rs:25-45` | current compatibility versions | E2, snapshot-current |
| `docs/doc-sync-audit-2026-04-29.md:185-210` | acknowledged deferred sync/design/research debt | E5, dated-aging |

**`[FACT]`** Samples were read across every major docs subtree, including:
`agent-reports/tool_call_processing_bug_report.md`,
`archive/2026-04-17-rescued/INDEX.md`, `audit/doc-sync-apr12-delta-checklist.md`,
`bugs/tui-pi-chat-launch-enoent.md`, `codex-gpt55-investigation/test-results.md`,
all three terminal decision-memo study families, `design/doc-sync-system.md`,
`designs/README.md`, `guides/install.md`,
`incidents/agent-949-reopen-owner-evidence.md`, `manual/README.md`,
`ops/runbook.md`, `pi-integration/integration-plan-v2.md`,
`plans/integrated-multi-user-roadmap.md`, both initial and follow-up
`prod-audit` syntheses, a recent worker-completion report, `research/README.md`,
`studies/task-lifecycle-coordinator-deep-survey.md`, terminal-bench reference,
and the trace/replay test specification. The one-file `assets/`, `poetry/`,
`prompts/`, and `probes/` surfaces were also read (`assets/README.md`,
`poetry/workgraph-poems.typ`, `prompts/selfhost.md`, and
`probes/codex-gpt-5.6-sol.md`). File-list and keyword searches covered the
remaining top-level/docs-root ADR, config, model, audit, manual, report,
research, design, plan, runbook, and archive surfaces.

### 7.4 What was not verified

**`[FACT]`** This leaf audit did not run the full Rust test suite, smoke suite,
manual generator, Typst/Pandoc, website build/deployment, installer, external
links, browser journeys, TUI, credentialed model/provider flows, federation,
review, provider, or pilot flows. It did not semantically compare every manual
or quickstart derivative. Test and report source was inspected unless an exact
command above is marked `[VERIFIED]`.

**`[UNCERTAINTY]`** Other thematic audit artifacts may narrow or supersede domain
examples here after deeper source/test execution. The downstream product/docs
synthesis should preserve this document's stable IDs, particularly the
scope-qualified contradictions, rather than generalizing them into claims that
all documentation or all represented behavior is stale.
