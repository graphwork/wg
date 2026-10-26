# Documentation Audit — Delta Checklist

**Date:** 2026-04-12
**Task:** doc-sync-apr12-doc-sync-mar28-ds4-doc-sync-ds3-ds2-doc-sync-spec
**Baseline:** CLI `wg --help-all` output, code in `src/`, current behavior

This audit compares each key documentation file against the current CLI commands,
code features, and runtime behavior. Each section lists what is stale, missing,
or inaccurate.

---

## 1. README.md (Project Overview)

**Status: Mostly current. Minor gaps.**

### Stale / Inaccurate
- (none found — README was recently synced)

### Missing
- [ ] `wg user` command not mentioned (per-user conversation boards)
- [ ] `wg profile` command not mentioned (provider profile presets)
- [ ] `wg spend` command not mentioned (token usage summaries)
- [ ] `wg openrouter` command not mentioned (OpenRouter cost monitoring)
- [ ] No mention of the `designs/` subdirectory (newer design docs live there)
- [ ] TUI section could mention the iteration navigator widget (recently added via `impl-iteration-navigator`)
- [ ] No mention of `--allow-cycle` flag on `wg check`

### Notes for downstream (doc-sync-readme)
- README is well-structured and comprehensive. Focus updates on new commands and TUI enhancements.

---

## 2. docs/COMMANDS.md (CLI Command Reference)

**Status: Comprehensive but has gaps for newest commands.**

### Missing Commands (in `--help-all` but not in COMMANDS.md)
- [ ] `wg user` — manage per-user conversation boards (`.user-NAME`)
- [ ] `wg profile` — manage provider profiles (model tier presets)
- [ ] `wg spend` — show token usage and estimated cost summaries
- [ ] `wg openrouter` — OpenRouter cost monitoring and management
- [ ] `wg requeue` — requeue an in-progress task for failed-dependency triage

### Possibly Stale
- [ ] `wg models` section lists subcommands `list`, `search`, `remote`, `add`, `set-default`, `init` — verify these all still match the code
- [ ] `wg check` — no mention of `--allow-cycle` flag (added in recent commit `2eaf982a`)

### Missing Flags on Existing Commands
- [ ] `wg add` — check if `--draft` flag exists (README mentions `--paused`, COMMANDS says `--paused`, need to verify `--draft` alias)
- [ ] `wg list` — verify `--pending-validation` filter documented correctly (status filter value may be `pending-validation`)
- [ ] `wg service start` — verify `--no-coordinator-agent` flag is documented (it is, line 2386)

### Notes for downstream (doc-sync-commands)
- Main gaps are 5 missing commands. These need full entries with examples.

---

## 3. docs/AGENT-GUIDE.md (Agent Patterns & Lifecycle)

**Status: Current. Well-maintained.**

### Stale / Inaccurate
- (none found — recently synced)

### Missing
- [ ] No mention of `wg msg read` `--agent` flag behavior and cursor semantics
- [ ] Chatroom pattern: `wg msg list` is used in examples but the actual subcommand is documented as a standalone section in COMMANDS.md — verify consistency
- [ ] No mention of the `wg user` board feature for human-in-the-loop scenarios
- [ ] Could mention `wg wait --until human-input` as an alternative to polling messages

### Notes for downstream (doc-sync-agent)
- Guide is solid. Incremental improvements around messaging and human-in-the-loop.

---

## 4. docs/AGENT-SERVICE.md (Service Architecture)

**Status: Good. Minor staleness in tick description.**

### Stale / Inaccurate
- [ ] Coordinator tick steps may have drifted — step 1.3 (zero-output detection) and step 2.8 (message-triggered resurrection) should be verified against current `coordinator.rs`

### Missing
- [ ] No mention of coordinator persistence across restarts (recently fixed, commit `cd8b3c07`)
- [ ] No mention of multi-coordinator support (`wg config --max-coordinators`)
- [ ] No mention of `wg service interrupt-coordinator` behavior
- [ ] Missing docs on the coordinator chat inbox processing (step 0 in tick)
- [ ] No mention of `--no-coordinator-agent` flag on `wg service start`

### Notes for downstream (doc-sync-agent)
- Focus on coordinator persistence, multi-coordinator, and tick step verification.

---

## 5. docs/AGENCY.md (Agency System)

**Status: Current. Comprehensive.**

### Stale / Inaccurate
- (none found)

### Missing
- [ ] No mention of `wg agency create` (creator agent for discovering primitives)
- [ ] No mention of `wg agency import` (CSV import)
- [ ] No mention of `wg agency stats --by-model` flag
- [ ] No mention of `wg agency deferred` / `approve` / `reject` commands
- [ ] Missing `wg profile` integration with agency (provider profiles)
- [ ] Skill system section could mention `wg skill install` more prominently

### Notes for downstream (doc-sync-agency)
- Agency doc is comprehensive on concepts. Missing CLI commands added since last sync.

---

## 6. docs/LOGGING.md (Logging & Provenance)

**Status: Current.**

### Stale / Inaccurate
- (none found)

### Missing
- [ ] No mention of `wg log --agent` flag for viewing agent prompts/outputs
- [ ] No mention of `wg log --operations` for operations log access

### Notes for downstream (doc-sync-manual)
- Minor additions only.

---

## 7. docs/DEV.md (Development Notes)

**Status: Mostly current.**

### Stale / Inaccurate
- [ ] Function table lists only `doc-sync` and `tfp-pattern` — likely more functions exist now, but the doc correctly says "Run `wg func list` for the full catalog"

### Missing
- [ ] No mention of `wg compact` for context distillation
- [ ] No mention of `wg sweep` for orphan recovery
- [ ] No mention of `wg spend` for cost tracking during development
- [ ] Worktree section could reference `docs/AGENT-LIFECYCLE.md` for the hardened lifecycle docs

### Notes for downstream (doc-sync-manual)
- Minor additions.

---

## 8. docs/WORKTREE-ISOLATION.md (Worktree Isolation)

**Status: Current but dated (Feb 2026 research report).**

### Stale / Inaccurate
- [ ] This is a research report, not operational docs. The operational content is now better covered in `docs/AGENT-LIFECYCLE.md` and `docs/DEV.md`

### Missing
- [ ] No mention of `wg cleanup orphaned` and `wg cleanup recovery-branches` commands for manual worktree cleanup
- [ ] No mention of the three-layer circuit breaker (zero-output detection)

### Notes for downstream (doc-sync-agent)
- Consider whether this doc should be archived or merged into AGENT-LIFECYCLE.md.

---

## 9. docs/models.md (Model/Endpoint/Key Management)

**Status: Current.**

### Stale / Inaccurate
- (none found)

### Missing
- [ ] No mention of `wg profile` command for provider profile presets
- [ ] No mention of `wg openrouter` cost monitoring command
- [ ] No mention of `wg spend` for token usage summaries

### Notes for downstream (doc-sync-commands)
- Model management is well-documented. Add new profile/spend/openrouter commands.

---

## 10. docs/MODEL_REGISTRY.md (Model Provider Registry)

**Status: Needs review.**

### Potentially Stale
- [ ] Model IDs and tier names may have changed since last edit (e.g., Claude model IDs updated to opus-4-6/sonnet-4-6)

### Missing
- [ ] No mention of `wg models` browse command
- [ ] No mention of `wg profile` presets

### Notes for downstream (doc-sync-commands)
- Cross-reference with `wg model list` output.

---

## 11. .claude/skills/wg/SKILL.md (Claude Code Skill)

**Status: Current.**

### Stale / Inaccurate
- (none found — well-maintained)

### Missing
- [ ] No mention of `wg user` boards
- [ ] No mention of `wg profile` command
- [ ] No mention of `wg spend` or `wg openrouter` for cost awareness

### Notes for downstream (doc-sync-skill)
- Minor additions for new commands.

---

## 12. src/commands/quickstart.rs (Quickstart Text)

**Status: Current and comprehensive.**

### Stale / Inaccurate
- (none found — recently updated)

### Missing
- [ ] No mention of `wg user` boards
- [ ] No mention of `wg profile` presets
- [ ] No mention of `wg spend` cost tracking

### Notes for downstream (doc-sync-quickstart)
- Very comprehensive already. Minor additions for newest commands.

---

## 13. docs/SECURITY.md (Security Guide)

**Status: Current.**

### Stale / Inaccurate
- (none found)

### Missing
- [ ] No mention of `wg key` commands for API key management
- [ ] Could cross-reference `docs/models.md` for secure key configuration

### Notes for downstream (doc-sync-manual)
- Minor cross-reference additions.

---

## 14. docs/AGENT-LIFECYCLE.md (Hardened Agent Lifecycle)

**Status: Current and detailed.**

### Stale / Inaccurate
- (none found)

### Missing
- [ ] No mention of coordinator persistence across restarts (recent fix)

### Notes for downstream (doc-sync-agent)
- Very thorough document. Minor updates only.

---

## 15. docs/agent-git-hygiene.md (Git Hygiene Rules)

**Status: Current.**

### Stale / Inaccurate
- (none found)

### Missing
- (none found — well-scoped document)

### Notes for downstream
- No changes needed.

---

## 16. docs/manual/ (Typst Manual Chapters)

**Status: Markdown versions may lag behind Typst sources.**

### Stale / Inaccurate
- [ ] Markdown versions (01-05) may be stale relative to .typ sources — need diff comparison
- [ ] Manual may not cover: `wg user`, `wg profile`, `wg spend`, `wg openrouter`, `wg requeue`, `wg compact`, `wg sweep`

### Missing
- [ ] Chapter on coordinator persistence and multi-coordinator support
- [ ] Chapter coverage of newest commands (user boards, profiles, cost tracking)

### Notes for downstream (doc-sync-manual)
- Compare .md vs .typ versions. Focus on command coverage gaps.

---

## 17. docs/guides/openrouter-setup.md

**Status: Needs verification.**

### Possibly Stale
- [ ] Setup steps may have changed with `wg profile` command introduction
- [ ] `wg openrouter` cost monitoring not mentioned

### Notes for downstream (doc-sync-commands)
- Verify against current `wg endpoints` and `wg openrouter` commands.

---

## 18. docs/guides/server-setup.md

**Status: Needs verification.**

### Possibly Stale
- [ ] Verify against current `wg server init` and `wg server connect` commands

### Notes for downstream (doc-sync-manual)
- Cross-reference with COMMANDS.md server section.

---

## Documents Missing from KEY_DOCS.md

The following files exist on disk but are NOT listed in the KEY_DOCS index:

### New Design Docs (not in KEY_DOCS.md)
- [ ] `docs/design/phantom-edge-prevention.md` — Phantom edge prevention design
- [ ] `docs/design/safe-coordinator-cycle.md` — Safe coordinator cycle design
- [ ] `docs/design/bare-coordinator.md` — Bare coordinator design
- [ ] `docs/design/coordinator-id-assignment.md` — Coordinator ID assignment design
- [ ] `docs/design/tui-multi-panel.md` — TUI multi-panel layout design (IS in KEY_DOCS but needs status check)
- [ ] `docs/design/design-autopoietic-task-agency.md` — Autopoietic task agency design
- [ ] `docs/design/native-graph-iteration.md` — Native graph iteration design

### New Design Directory (`docs/designs/`)
- [ ] `docs/designs/chat-message-ordering-and-delivery.md` — Chat message ordering design
- [ ] `docs/designs/failed-dep-triage.md` — Failed dependency triage design
- [ ] `docs/designs/quality-pass.md` — Quality pass design
- [ ] `docs/designs/tui-iteration-history-and-viz-selfloop.md` — TUI iteration history design

### New Plan Docs (not in KEY_DOCS.md)
- [ ] `docs/plans/user-board-design.md` — User board design plan
- [ ] `docs/plans/spiral-unrolling-design.md` — Spiral unrolling design plan
- [ ] `docs/plans/assignment-time-placement-guard.md` — Assignment-time placement guard
- [ ] `docs/plans/model-registry-and-update-trace.md` — Model registry update trace
- [ ] `docs/plans/provider-profiles.md` — Provider profiles plan

### New Report Docs (not in KEY_DOCS.md)
- [ ] `docs/reports/bug-report-user-board-leak.md` — Bug report: user board leak
- [ ] `docs/reports/smoke-test-cycle-lifecycle.md` — Cycle lifecycle smoke test
- [ ] `docs/reports/triage-task-naming-investigation.md` — Triage task naming investigation
- [ ] `docs/reports/research-coordinator-chat-ordering.md` — Coordinator chat ordering research
- [ ] `docs/reports/openrouter-new-repo-setup-guide.md` — OpenRouter new repo setup guide
- [ ] `docs/reports/bug-report-assign-task-not-blocking.md` — Bug: assign task not blocking

### New Research Docs (not in KEY_DOCS.md)
- [ ] `docs/research/primitive-pool-location.md` — Primitive pool location research
- [ ] `docs/research/agency-primitive-sync-model.md` — Agency primitive sync model
- [ ] `docs/research/tui-inspector-panel-resizing.md` — TUI inspector panel resizing
- [ ] `docs/research/spiral-cycle-unrolling-gap-analysis.md` — Spiral cycle unrolling gap analysis
- [ ] `docs/research/iterate-vs-retry-design.md` — Iterate vs retry design
- [ ] `docs/research/openrouter-leaderboard-api.md` — OpenRouter leaderboard API
- [ ] `docs/research/profile-research.md` — Profile research
- [ ] `docs/research/wg-config-profiles.md` — Config profiles research
- [ ] `docs/research/evolve-yaml-cache-paths.md` — Evolve YAML cache paths
- [ ] `docs/research/config-structure-and-setup.md` — Config structure and setup
- [ ] `docs/research/stuck-detection-research.md` — Stuck detection research
- [ ] `docs/research/thinking-token-patterns.md` — Thinking token patterns
- [ ] `docs/research/tb-autopoietic-integration.md` — TB autopoietic integration
- [ ] `docs/research/litellm-executor-fallback-analysis.md` — LiteLLM executor fallback
- [ ] `docs/research/supervisor-agent-loop.md` — Supervisor agent loop
- [ ] `docs/research/phantom-edge-analysis.md` — Phantom edge analysis
- [ ] `docs/research/shell-executor-and-retry-patterns.md` — Shell executor retry patterns
- [ ] `docs/research/native-executor-compact-messages-pattern.md` — Native executor compact messages
- [ ] `docs/research/existing-design-documents-journal-compaction.md` — Existing design docs on journal compaction

### New Audit/Other Docs
- [ ] `docs/audit/agent-work-integrity.md` — Agent work integrity audit
- [ ] `docs/SECURITY.md` — Security guide (listed in KEY_DOCS but verify it's there)
- [ ] `docs/design-shell-executor.md` — Shell executor design
- [ ] `docs/terminal-bench/DESIGN-native-executor-improvements.md` — Terminal bench: native executor improvements
- [ ] `docs/terminal-bench/REFERENCE-terminal-bench-campaign.md` — Terminal bench: campaign reference
- [ ] `docs/terminal-bench/REVIEW-doc-analysis.md` — Terminal bench: doc analysis review
- [ ] `docs/terminal-bench/ROADMAP-terminal-bench.md` — Terminal bench: roadmap

---

## Summary of Cross-Cutting Gaps

### Commands Missing from All Docs
These commands exist in `wg --help-all` but are not documented in ANY user-facing doc:
1. **`wg user`** — per-user conversation boards
2. **`wg profile`** — provider profile presets
3. **`wg spend`** — token usage and cost summaries
4. **`wg openrouter`** — OpenRouter cost monitoring
5. **`wg requeue`** — requeue in-progress tasks for triage

### Features Under-Documented
1. **Coordinator persistence** — recent fix preserving tasks across service restart
2. **Multi-coordinator support** — `--max-coordinators` config
3. **TUI iteration navigator** — recently added widget
4. **`wg check --allow-cycle`** — new flag for cycle detection tests

### Structural Issues
1. **`docs/designs/`** directory exists alongside `docs/design/` — inconsistent naming
2. **`docs/terminal-bench/`** directory not indexed in KEY_DOCS.md
3. **`docs/audit/`** directory not indexed in KEY_DOCS.md
4. **KEY_DOCS.md** is 30+ docs behind the actual file count

---

## Priority Matrix (for downstream tasks)

| Priority | Area | Delta Count | Downstream Task |
|----------|------|-------------|-----------------|
| **HIGH** | COMMANDS.md — 5 missing commands | 5 | doc-sync-commands |
| **HIGH** | KEY_DOCS.md — 30+ missing entries | 30+ | (this task) |
| **MED** | AGENT-SERVICE.md — coordinator updates | 5 | doc-sync-agent |
| **MED** | AGENCY.md — new CLI commands | 5 | doc-sync-agency |
| **MED** | README.md — new commands | 5 | doc-sync-readme |
| **LOW** | Manual chapters — .md/.typ sync | TBD | doc-sync-manual |
| **LOW** | SKILL.md — minor additions | 3 | doc-sync-skill |
| **LOW** | Quickstart — minor additions | 3 | doc-sync-quickstart |
