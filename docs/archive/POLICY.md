# Documentation archive policy

**Status:** in force. This file is the policy; `scripts/doc-archive-audit.sh` is
its mechanical gate. It answers two questions: *may this doc be archived?* and
*how do I archive it without breaking a citation?*

Established by task `doc-archive-policy` from
`docs/survey/survey-synthesis-subtraction-plan.md` §1 S21 and
`.wg/survey/survey-tests-docs.md` §4 C4 ("Archive stale docs under
`docs/archive/`"), with the keep list from the synthesis §3 (`do-not-delete`).

---

## 1. Why this exists

| Measure (2026-10-06) | Value |
|---|---|
| Markdown files in the repo | 780 tracked |
| `docs/` markdown | 624 files / 14.3 MB on disk |
| Docs with **no inbound markdown link** | 91 (survey) |
| Markdown at the **repo root** | 56 (`README`/`AGENTS`/`CLAUDE` + **53** one-off reports) |
| `docs/KEY_DOCS.md` last verified | 2026-04-29 — stale by the whole federation/review/exec/pilot wave |
| Docs churn | 224 commits / 60 days |
| Commit history | **never** deleted, always preserved |

Archiving is a *navigation* fix, not a disk fix. A 14 MB `docs/` tree is cheap;
an un-signposted `docs/` tree is expensive in grep noise, in stale-path citations
from source, and in "which of these two design docs is true?" questions. The
policy therefore optimises for **the live surface staying small and honest**,
never for bytes.

**Archiving is never deletion.** Every move is `git mv`, so `git log --follow`
still walks the file's history and `git revert` restores it.

## 2. The keep / archive rule

A markdown file is **KEEP** if *any* of these is true; otherwise it is
**ARCHIVE-ELIGIBLE**:

- **K1 — Something load-bearing names it.** A Rust/TS/JS/Python source file, a
  script under `scripts/` or `tests/`, CI (`.github/`), the `Makefile`, or a
  config file cites the path or the basename. A code comment that cites a design
  doc **is** a citation. (`scripts/doc-archive-audit.sh` verdict **BLOCK**.)
- **K2 — A checked-in policy surface names it as canonical.** `README.md`,
  `AGENTS.md` / `CLAUDE.md`, `docs/KEY_DOCS.md`, `docs/ops/runbook.md`,
  `docs/guides/*`, or a `src/text/*` bundled doc lists it as the doc for a
  surface that still exists. This is a human decision, *not* something the audit
  script can decide: the script only tells you the file is referenced.
- **K3 — It is a decision of record still in force.** An ADR
  (`docs/ADR-*.md`) whose decision has not been superseded, or the design doc for
  behaviour that ships today. A *superseded* ADR may be archived **only** when
  its successor is written, the ADR index/series brief points at the successor,
  and the audit reports no code citation.
- **K4 — It documents a live operator/user surface.** Anything a user or operator
  runs today: `docs/COMMANDS.md`, `docs/guides/*`, `docs/ops/*`, `docs/manual/*`,
  `docs/bugs/*`, and the runbook.
- **K5 — It is a spark-stage subsystem doc.** `docs/spark/*` — off the daily
  path, but live and referenced by `AGENTS.md` and `docs/KEY_DOCS.md`.

**ARCHIVE-ELIGIBLE** means it is a *point-in-time artefact*:

- a dated audit / report / triage / investigation / status / closeout for work
  that is finished or abandoned;
- a plan or design for work that landed (the design of record for *live*
  behaviour is K3, but the "*plan to do it*" is archival);
- a one-off research note whose conclusions have been folded into a live doc;
- a superseded draft or duplicate.

The tie-break: **if you cannot name the live doc a reader should use instead,
the file is not archive-eligible yet** — write the replacement first.

## 3. Never archived (unconditional keep)

- `README.md`, `AGENTS.md`, `CLAUDE.md` (repo root is a seven-file surface:
  these three plus the four source-cited design docs listed in §4).
- `docs/KEY_DOCS.md` and this policy.
- `docs/COMMANDS.md`, `docs/AGENT-*.md`, `docs/agent-git-hygiene.md`,
  `docs/WORKTREE-ISOLATION.md`, `docs/config-*.md`, `docs/models.md`,
  `docs/SECURITY.md`, `docs/DEV.md`, `docs/LOGGING.md`, `docs/AGENCY.md`,
  `docs/AGENT-LIFECYCLE.md`, `docs/AGENT-SERVICE.md`.
- `docs/ops/*`, `docs/guides/*`, `docs/manual/*`, `docs/bugs/*`.
- `docs/ADR-*.md` (unless a superseded-ADR case per K3 is fully satisfied).
- `docs/spark/*` while the corresponding modules exist in `src/`.
- **Completion / validation policy docs** — anything a fail-closed gate reads
  (`docs/ops/completion-validation-evidence.md`, the review-gate ADR series).
  Never archived, even when the feature is quiet.

## 4. The repo root is a seven-file surface

`README.md`, `AGENTS.md`, `CLAUDE.md`, and the four root docs that **source
cites by name** (audit verdict **BLOCK**, they cannot move without a code edit):

| File | Cited by |
|---|---|
| `context.md` | `src/config.rs`, `src/commands/service/coordinator_agent.rs`, `.claude/skills/wg/SKILL.md` |
| `design-claim-lifecycle.md` | `src/commands/claim_lifecycle.rs` |
| `PROMPT_CONSTRUCTION_ANALYSIS.md` | `src/commands/deliverables.rs`, `tests/integration_worktree_observer.rs` |
| `prompt_construction_analysis.md` | `tests/integration_worktree_observer.rs` |

New root-level reports are not allowed. Write the doc under `docs/reports/`
(if it is a report) or `docs/design/` (if it is a design), and let this policy
handle it later.

## 5. Procedure: grep before you archive

```bash
# 1. Ask the gate. Never skip this — a BLOCK is a citation, not noise.
scripts/doc-archive-audit.sh path/to/doc.md
#   BLOCK  -> do not archive; repoint the code reference first
#   WARN   -> archive allowed, but repoint the prose references in THIS commit
#   CLEAR  -> archive now

# 2. Batch question: "these N files move together, ignore refs between them"
scripts/doc-archive-audit.sh --batch --orphans docs/reports

# 3. Move (git mv only — never rm, never cp+rm)
mkdir -p docs/archive/reports
git mv docs/reports/foo.md docs/archive/reports/foo.md

# 4. Re-run the gate over the archive destination: it must not report the moved
#    path from any code file. WARN from a historical audit report is acceptable
#    and is recorded below rather than rewritten (see §6).
mkdir -p docs/archive/reports
scripts/doc-archive-audit.sh docs/archive/reports/foo.md

# 5. Update the inbound prose links you just broke (same commit):
#    - relative link  -> ../<bucket>/foo.md
#    - inline-code mention in a LIVE doc -> docs/archive/<bucket>/foo.md
#    - inline-code mention in a HISTORICAL audit/report -> leave it (§6)

# 6. Record the move in the wave table below, then commit with the moved file
#    and every repointed link in one commit (git records the rename).
```

### ADRs specifically (the rule that matters most)

ADRs are the docs most likely to be cited from source and least safe to move:

1. `scripts/doc-archive-audit.sh docs/ADR-fed-002-transport.md` — a **BLOCK**
   from `src/identity/transport.rs` means the ADR is the decision of record for
   shipping code: **keep it, permanently**.
2. Archive a superseded ADR only after: (a) the successor ADR exists and its
   series brief (`docs/ADR-*-000-acceptance-brief.md`) points at it, (b) the
   audit is CLEAR or WARN-only, (c) the series brief's table row is updated in
   the same commit.
3. An ADR is **never** archived because it is old, quiet, or about a feature
   nobody uses. Decision records do not expire; they explain *why*.

## 6. Historical prose is not rewritten

An audit report that measured "the root has `action-plan-2026-04-14.md`" is a
record of a measurement at a point in time. After the file moves, that mention
is left alone: rewriting it would falsify the record, and the audit itself is
archive-eligible anyway. The old → new resolution is the git rename plus this
policy's wave table. Live docs (`notes/`, `docs/bugs/`, guides, runbook,
`docs/*.md` outside `archive/`) **are** repointed in the same commit.

## 7. Layout

```
docs/archive/
├── POLICY.md                     # this file (never archived)
├── root-reports/                 # one-off reports that used to sit at the repo root
├── research/                     # historical research notes
├── reports/                      # historical dated reports / closeouts
├── audit/                        # historical audits (incl. dated audit trees)
├── superseded-designs/           # designs superseded by a live doc
├── adr/                          # superseded ADRs only (see §5)
├── reviews/                      # pre-existing review buckets (2026-04)
└── 2026-04-17-rescued/           # pre-existing rescue bucket (2026-04-17)
```

Rules for the layout: **one bucket per document kind**, flat inside the bucket
(moved trees keep their internal shape so their sibling links still resolve),
original filename preserved so `git log --follow` and old citations stay
recognisable. A new bucket needs a row in the wave table below.

## 8. Waves

### Wave 1 — 2026-10-06 (this task): root reports

Before: 56 markdown files at the repo root. After: **7** (§4).
Gate: `scripts/doc-archive-audit.sh --root-reports` →
**53 candidates — 4 BLOCK, 8 WARN, 41 CLEAR**; with `--batch` over the 49 movers →
**0 BLOCK, 0 WARN, 49 CLEAR**.

| Verdict | Files | Outcome |
|---|---:|---|
| CLEAR | 41 | `git mv` → `docs/archive/root-reports/` |
| WARN → CLEAR under `--batch` | 8 | moved (4 were only cited by each other; 4 cited by historical audits — see §6; 1 live `notes/` mention repointed) |
| BLOCK | 4 | kept at root (§4) |

Inbound links repaired in the same commit: `action-plan-2026-04-14.md` ↔
`issues-2026-04-14.md` (relative links between the two archive buckets, both of
which were **already broken** before the move), and the
`notes/fix-android-firefox-wg-html-font-fallback.md` inline mention.

### Wave 2 — measured, not yet done (`doc-archive-policy-2` follow-up)

Counts from `scripts/doc-archive-audit.sh --batch --orphans <dir>` on the same
commit, i.e. "what is archivable *after* the directory's own internal
cross-references stop counting":

| Directory | Files | BLOCK (keep) | WARN (needs link repair) | CLEAR today |
|---|---:|---:|---:|---:|
| `docs/reports/` | 96 | 1 | 36 | 59 |
| `docs/research/` | 121 | 10 | 98 | 13 |
| `docs/audit/` | 23 | 1 | 7 | 15 |
| `docs/design/` | 82 | 5 | 77 | 0 |
| `docs/plans/` | 13 | 1 | 11 | 1 |
| `docs/designs/` | 7 | 1 | 4 | 2 |
| `docs/prod-audit/` | 7 | 4 | 3 | 0 |

Two things that table says, and that the follow-up should act on:

1. **`docs/reports/` and `docs/audit/` are the cheap wins** — 74 files with no
   code citation. `docs/research/` is next but 98 files carry prose references
   (mostly from each other and from `docs/KEY_DOCS.md`).
2. **`docs/design/` is not archival mass.** 77 of 82 files are cross-referenced
   *by other design docs*; these are the design of record for live behaviour
   (K3), not stale reports. Do not bulk-move them; apply K3 per file.
3. Regenerating `docs/KEY_DOCS.md` (wave 1, see §9) removes a large share of the
   remaining WARNs at once, because the stale hand-maintained list is itself one
   of the biggest referrers.

## 9. `docs/KEY_DOCS.md` is generated now

The hand-maintained 32 KB list went stale on 2026-04-29 and stayed stale through
an entire feature wave, so it is a **generated** artefact from this policy on:

```
docs/key-docs.tsv   # curated: zone, path, purpose, audience  (the human input)
scripts/gen-key-docs.py   # renders docs/KEY_DOCS.md + the census/archive pointers
```

- Curated zones (`core`, `agent`, `user`, `operator`, `adr`, `spark`, `design`)
  are listed with purpose and audience; everything else is reported as a
  **census** (per-directory file counts) rather than enumerated, so the file
  stays readable as the tree grows.
- Every path in the TSV is **existence-checked**; a missing path fails the
  generator loudly instead of silently rotting.
- `scripts/gen-key-docs.py --check` re-renders and fails on drift, so a doc
  added without being registered is a one-command `--check` away from being
  noticed (CI can wire this exactly like the `pi-worksgood` embed-staleness job).

## 10. Restoring a doc

```bash
git log --follow --oneline docs/archive/root-reports/foo.md   # find the move commit
git mv docs/archive/root-reports/foo.md docs/foo.md           # move it back
```

Then re-run the gate on the restored path, add it to `docs/key-docs.tsv` if it
is canonical, and re-render `docs/KEY_DOCS.md`.
