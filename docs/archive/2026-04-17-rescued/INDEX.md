# Rescued from dangling commit eef15157 — 2026-04-17

## Provenance

These files were accidentally committed in `eef15157` on 2026-04-17
(an unintentional `git add -A` swept ~14,000 untracked files into a
single commit), then removed when that commit was reverted and the
history rewritten to drop both the bad commit and its revert.

The content itself is **real engineering work** — documentation
audits, design notes, root-cause analyses, a security hardening
proposal, a worktree-damage code study — produced by agents or by
the user over prior sessions in the working tree but never properly
filed under `docs/`. Rather than lose it to the git GC horizon
(unreachable objects are pruned after ~14 days), we rescued the
clearly-valuable subset here.

What was **not** rescued:

- `bug_report_*.md` — our analysis earlier this session showed these
  were agent hallucinations triggered by the web_fetch-path confusion
  (agent couldn't find citations because it was reading the wrong
  log file, not because citations were missing).
- `analysis.md` / `blog_summary.md` / `markdown_examples.md` —
  generic-named scratch.
- Creative outputs: `haiku_*.txt`, `tokyo_sushi_recommendations.md`,
  `reflection_on_memory_computation.md`, `self_reflection_*.md`,
  `deep_reflection_journaling_compaction.md`.
- Ephemeral benchmark inputs: `test_*_tokens*.txt`,
  `pride_and_prejudice_sample.txt` (my own smoke test today).
- CLI dumps (`cli-commands-help-all.txt`, `cli-commands-list.txt`) —
  regenerable from `wg help`.
- Stray/duplicate source: `web_search.rs` at repo root (already in
  `src/executor/native/tools/web_search.rs`).
- Placeholder docs: `raft_papers.md` (18 lines, skeleton).
- The 13,640 YAML files in `terminal-bench/tb-results/...` —
  per-trial benchmark output, per-run artifacts, intentionally
  excluded via `.gitignore` going forward.

Anything below is available here. If you'd rather resurrect a file
NOT listed here (e.g. decide a bug_report_*.md is actually useful),
it's still reachable via `git show eef15157:path/to/file` for the
next ~14 days (until git's unreachable-object GC fires).

## Groups

### Security — native executor

- [native-executor-hardening-analysis.md](native-executor-hardening-analysis.md) —
  **Most directly relevant to current work.** 269-line analysis of
  native-executor security vulnerabilities (command injection, FS
  access, SSRF, resource exhaustion) with a phased hardening
  roadmap and a proposed `.wg/security.toml` configuration
  schema.

- [security-remediation-design.md](security-remediation-design.md) —
  Broader security-remediation design.

- [security-remediation-complete-summary.md](security-remediation-complete-summary.md) —
  Summary of the above.

### Worktree lifecycle — root cause analyses

- [worktree_lifecycle_analysis.md](worktree_lifecycle_analysis.md)
- [root_cause_analysis_worktree_collisions.md](root_cause_analysis_worktree_collisions.md)
- [root-cause-analysis-final.md](root-cause-analysis-final.md)
- [agent-exit-worktree-cleanup-audit.md](agent-exit-worktree-cleanup-audit.md)
- [worktree-damage-study/](worktree-damage-study/) — source files
  and patch from a hands-on study of the worktree damage fix.

### Coordinator — persistence + chat research

- [coordinator-persistence-analysis.md](coordinator-persistence-analysis.md)
- [coordinator-chat-research-findings.md](coordinator-chat-research-findings.md)

### Iteration Navigator (TUI design)

- [iteration-navigator-design.md](iteration-navigator-design.md)
- [iteration-navigator-data-flow.md](iteration-navigator-data-flow.md)
- [iteration-navigator-interaction-spec.md](iteration-navigator-interaction-spec.md)
- [tui-iteration-research-summary.md](tui-iteration-research-summary.md)

### Manual / documentation audit (April 2026)

A multi-session audit of the user manual and CLI documentation.
Several sections, a synthesis, an inventory, and a checklist.

- [manual-audit-inventory-2026-04-12.md](manual-audit-inventory-2026-04-12.md)
- [audit-manual-section-1-findings.md](audit-manual-section-1-findings.md)
- [audit-manual-section-2-findings.md](audit-manual-section-2-findings.md)
- [audit-manual-section-3-findings.md](audit-manual-section-3-findings.md)
- [manual-section-4-coordination-audit-report.md](manual-section-4-coordination-audit-report.md)
- [audit-manual-supporting-findings.md](audit-manual-supporting-findings.md)
- [unified-manual-audit-findings.md](unified-manual-audit-findings.md)
- [comprehensive-manual-audit-synthesis.md](comprehensive-manual-audit-synthesis.md)
- [evolution-manual-audit-findings.md](evolution-manual-audit-findings.md)
- [consolidated-documentation-audit-checklist.md](consolidated-documentation-audit-checklist.md)
- [canonical-key-docs-index.md](canonical-key-docs-index.md)
- [design-documentation-audit-report.md](design-documentation-audit-report.md)
- [research-documentation-audit-report.md](research-documentation-audit-report.md)

### CLI documentation coverage

- [cli-documentation-coverage-report.md](cli-documentation-coverage-report.md)
- [cli-documentation-gap-analysis.md](cli-documentation-gap-analysis.md)
- [final-cli-coverage-analysis.md](final-cli-coverage-analysis.md)
- [documented-commands-from-commands-md.txt](documented-commands-from-commands-md.txt)

### Research tool — enhancement proposal

- [research_enhancement_proposal.md](research_enhancement_proposal.md)
- [investigation_wg_limitations.md](investigation_wg_limitations.md)

### Task lifecycle

- [task-unclaiming-verification.md](task-unclaiming-verification.md)

### Issue tracking

- [issues-2026-04-14.md](issues-2026-04-14.md)

### Historical patches

- [damage-diff-2026-04-14.patch](damage-diff-2026-04-14.patch) —
  2469-line patch from a worktree-damage fix.

## Next actions (if useful)

- `native-executor-hardening-analysis.md` and the worktree-damage
  materials probably want to move up into `docs/design/` or
  `docs/security/` as first-class docs once reviewed. They're here
  as a holding area — not the permanent home.
- The manual-audit set is a set of working notes, not something
  users should read directly. Candidates for distillation into a
  single "manual changes applied" record in `docs/manual/` and
  then retirement.
