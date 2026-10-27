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

### Wave 2 — 2026-10-26 (`doc-archive-policy-2`): `docs/reports/` + `docs/audit/`

Before: `docs/reports/` **96** markdown files, `docs/audit/` **23**.
After: **16** and **7** (see the outcome table).

Gate, run per directory exactly as §5 step 2 prescribes (`--batch` so the
bucket's own internal cross-references stop counting):

```
scripts/doc-archive-audit.sh --batch --orphans docs/reports
  -> 96 candidate(s) — 1 BLOCK, 15 WARN, 80 CLEAR
scripts/doc-archive-audit.sh --batch --orphans docs/audit
  -> 23 candidate(s) — 1 BLOCK,  6 WARN, 16 CLEAR
scripts/doc-archive-audit.sh --batch --orphans docs/reports docs/audit   # both together
  -> 119 candidate(s) — 2 BLOCK, 21 WARN, 96 CLEAR
```

| Verdict | Files | Outcome |
|---|---:|---|
| CLEAR | 96 | `git mv` → `docs/archive/reports/` (80) and `docs/archive/audit/` (16), subdirectory shape preserved (`docs/reports/negative-control-gate/`, `docs/audit/2026-08-08-worksgood-system/`) |
| WARN | 21 | **left in place** — every referrer is a *live* doc (`docs/design*`, `docs/plans/*`, `docs/research/*`, `docs/doc-sync-audit-*.md`). Repointing those live docs is the next wave's decision, not this one's; per the §2 tie-break a file a live doc still points at is not archive-eligible yet. |
| BLOCK | 2 | kept at their original locations (below) |

114 files moved in total: the 96 markdown candidates plus the 18-file evidence
bundle `docs/reports/negative-control-gate/` (`*.txt` / `*.json` receipts), which
travels with its report because the report's own evidence mentions are
*relative* (`negative-control-gate/03-rejection-output.txt`) and would stop
resolving if the bundle were split. `docs/reports/recovery-loop-live-proof.md`
moved **alone**: its evidence tree `docs/reports/recovery-loop-live-proof/` holds
no markdown and stays put, and the report cites it repo-relatively
(`docs/reports/recovery-loop-live-proof/…`), so those mentions still resolve
unchanged.

The two BLOCK keepers, named by the gate:

| File | Cited by |
|---|---|
| `docs/reports/bug-report-assign-task-not-blocking.md` | `docs/research/traces/wg-control-plane-authority-deletion-map/07-assignment-plumbing-completion.json` |
| `docs/audit/2026-08-08-worksgood-system/README.md` | basename collision only — `README.md` matches every README in the tree (the audit-tree charter is a `.json`/`Cargo.toml` false positive, not a real citation). Kept in place because the gate says BLOCK; retiring it needs a gate fix, not a move. |

Inbound/outbound links repaired in the same commit (all of them mechanical, none
rewriting a historical *measurement* per §6):

- **69 links inside the moved files** — every link whose target stayed behind:
  sibling links to the WARN members of a split tree
  (`11-orchestration-lifecycle.md`, `23-…`, `30-…`, `31-…`, `99-SYNTHESIS.md`,
  `README.md`), links into the live tree one level up (`../../src/…` →
  `../../../src/…`, `../guides/…` → `../../guides/…`, `../../formal/…` →
  `../../../formal/…`), and links to WARN reports that stayed
  (`executor-arena-smoke.md`, `amplifier-research-report.md`,
  `inventory-luca-casa-stream-2026-07-18.md`).
- **83 links in 3 retained files** — the reverse direction: the retained half of
  the split audit tree (`99-SYNTHESIS.md`, `31-documentation-sync-plan.md`) and
  `docs/reports/inventory-luca-casa-stream-2026-07-18.md` pointing at the files
  that moved. They now resolve through `../../archive/audit/…` and
  `../../archive/reports/…`.
- Prose (inline-code) mentions are **not** rewritten (§6): `99-SYNTHESIS.md`,
  `30-contradiction-and-drift-register.md` and the audit charter still *name* the
  archived leaves; the git rename plus this table is the resolution.
- Net effect on link health, measured over every tracked `*.md` and every
  relative markdown link in the repo: **39 broken before the move, 39 after** —
  i.e. this wave introduced no new dangling link and fixed none of the
  pre-existing ones (out of scope).

The split of `docs/audit/2026-08-08-worksgood-system/` (14 moved, 6 kept) is the
gate's verdict, not an editorial choice: the six are cited by
`docs/plans/2026-08-09-simple-local-wg-recovery.md` and
`docs/design-adaptive-agency-loop.md`, so they are WARN and stay. Collapsing the
split means repointing those two live docs first (a wave-3 candidate).

### Wave 3 — measured, not yet done

Counts from `scripts/doc-archive-audit.sh --batch --orphans <dir>`, i.e. "what is
archivable *after* the directory's own internal cross-references stop counting".
The `docs/reports/` and `docs/audit/` rows are re-measured *after* wave 2:

| Directory | Files | BLOCK (keep) | WARN (needs link repair) | CLEAR today |
|---|---:|---:|---:|---:|
| `docs/reports/` | 16 | 1 | 15 | 0 |
| `docs/audit/` | 7 | 1 | 6 | 0 |
| `docs/research/` | 121 | 10 | 98 | 13 |
| `docs/design/` | 82 | 5 | 77 | 0 |
| `docs/plans/` | 13 | 1 | 11 | 1 |
| `docs/designs/` | 7 | 1 | 4 | 2 |
| `docs/prod-audit/` | 7 | 4 | 3 | 0 |

Three things that table says, and that the follow-up should act on:

1. **`docs/reports/` and `docs/audit/` are drained of everything the gate could
   move** — 117 of 119 files are archived; what is left is 21 WARN files whose
   referrers are live docs, plus the 2 BLOCK keepers. The cheap part is done.
   (The pre-wave-2 estimate in this policy said 36 WARN / 59 CLEAR for
   `docs/reports/`; re-measuring at execution time gave 15 WARN / 80 CLEAR,
   because wave 1's `docs/KEY_DOCS.md` regeneration + root-report move retired the
   biggest prose referrer. Measure, then move — never move from a stale table.)
2. **`docs/research/` is next** but 98 files carry prose references (mostly from
   each other and from `docs/KEY_DOCS.md`).
3. **`docs/design/` is not archival mass.** 77 of 82 files are cross-referenced
   *by other design docs*; these are the design of record for live behaviour
   (K3), not stale reports. Do not bulk-move them; apply K3 per file.

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
  noticed. It is wired into CI as the **`docs/KEY_DOCS.md is current`** job
  (`.github/workflows/ci.yml`), exactly like the `pi-worksgood` embed-staleness
  job: the job runs `--check`, so a stale index fails the build with the drift
  diff instead of rotting silently.
- The census is live from `git ls-files` (index state), so **run the generator in
  the same commit that adds, moves, or archives a doc** — `git add` first, then
  `scripts/gen-key-docs.py`, then commit both. Forgetting is caught, not
  punished: the `docs/KEY_DOCS.md is current` CI job and
  `tests/smoke/scenarios/doc_archive_policy.sh` both run the same `--check` and
  fail with the exact drift diff.

## 10. Restoring a doc

```bash
git log --follow --oneline docs/archive/root-reports/foo.md   # find the move commit
git mv docs/archive/root-reports/foo.md docs/foo.md           # move it back
```

Then re-run the gate on the restored path, add it to `docs/key-docs.tsv` if it
is canonical, and re-render `docs/KEY_DOCS.md`.
