# Unified Branch Audit Report

**Date:** 2026-03-12
**Branches audited:** 11
**Auditor:** agent-8377 (Reviewer), synthesizing reports from agents 8333–8369

---

## Executive Summary

11 branches were audited against current `main`. Of these:
- **1 branch** is ready to merge with minor conflict resolution (tui-disable-fade)
- **1 branch** contains a critical correctness fix that must be landed (fix-toctou-race — TOCTOU vulnerability)
- **3 branches** contain high-value features worth cherry-picking or reimplementing (fix-before-edges, show-live-token, tui-pink-lifecycle)
- **2 branches** contain a mix of valuable and superseded work requiring selective extraction (fix-output-section, fix-auto-task-edges)
- **2 branches** are fully superseded and should be deleted (infra-fix-toctou, provenance-and-executor-generalization)
- **2 external branches** (nikete/*) contain valuable research docs but all code is superseded

---

## Branch Overlap Map

Several branches share commits and have ancestor/descendant relationships:

```
                    fix-output-section (30 unique commits)
                         │
                    infra-fix-toctou = fix-output-section + de17801 (TOCTOU fix)
                         │
                    fix-toctou-race (59 unique commits, includes 43b3c7a from tui-disable-fade)

                    show-live-token (10 unique commits)
                         │
                    fix-before-edges = show-live-token + 1e333ee (before-edge normalization)

                    tui-disable-fade: commit 43b3c7a also on fix-toctou-race
```

**Key implication:** infra-fix-toctou is fully subsumed by fix-toctou-race. show-live-token is fully subsumed by fix-before-edges. The tui-disable-fade fix exists on fix-toctou-race but should be cherry-picked independently for smaller blast radius.

---

## Section 1: Branches to Merge / Cherry-Pick

### PRIORITY 1 — CRITICAL: TOCTOU Fix (from fix-toctou-race)

**Branch:** fix-toctou-race (59 commits, 124 files changed)
**Status:** Main is currently vulnerable to a TOCTOU race in graph mutations. With concurrent agents, silent data loss (overwritten mutations) is expected.
**Core fix:** `mutate_graph()` — closure-based API holding flock across entire read-modify-write cycle.

**Recommendation:** Do NOT merge the full branch (39 conflict files). Instead, phased cherry-pick:
1. **Phase 1:** Core `mutate_graph()` + parser refactor (b3ada7a) — highest priority
2. **Phase 2:** Convert all commands to `mutate_workgraph` (39e1651)
3. **Phase 3:** Convert remaining call sites (442556e)
4. **Phase 4:** Safety commands (wg retract, cascade-stop, hold/unhold, liveness, ask)
5. **Phase 5:** TUI improvements (dangling dep edges, auto-focus, temporal navigation)

Additional valuable commits (51/59 are valuable):
- `wg retract` — provenance-based undo
- `wg cascade-stop` / `wg hold` / `wg unhold` — subtree control
- `wg liveness` — agent health monitoring
- `wg ask` — interactive prompting
- Self-healing remediation for failed tasks
- Dangling dependency edges rendered in red
- Verify task split (verify-build vs verify-test)
- Safety operations integration tests

**Superseded (skip):** Compactor MVP (already on main), duplicate eval_timeout, 5 merge commits.

---

### PRIORITY 2 — Merge: tui-disable-fade

**Branch:** tui-disable-fade (1 commit: 43b3c7a)
**Status:** Small, clean, high-value. Removes unnecessary `Revealed` animation, improves cursor/scroll stability on toggle.
**Conflicts:** 1 trivial conflict in state.rs (keep main's SlideDirection, remove Revealed arm).
**Recommendation:** **Merge directly** after resolving the single conflict. Net -108 lines (removes complexity).

---

### PRIORITY 3 — Cherry-pick: fix-before-edges (before-edge normalization)

**Branch:** fix-before-edges (11 commits, superset of show-live-token)
**Key commit:** 1e333ee — Before-edge normalization (before edges invisible to readiness checks — real bug).

Valuable commits from this branch (which includes all of show-live-token):
1. **1e333ee** — Before-edge normalization into after edges (HIGH — bug fix)
2. **c6c1a6f** — Live token count display in TUI (HIGH — UX)
3. **4110ca3** — Streaming coordinator response to TUI (HIGH — UX)
4. **10fe502** — Verify string validation + circuit breaker (MEDIUM — safety)
5. **e1429b1** — Detail view collapse toggle + tail preview (MEDIUM — UX)
6. **ffa1e54** — Default agent assignment fallback (LOW-MEDIUM)
7. **000b4cb** — TUI config panel tests — 1195 lines (LOW but valuable coverage)

**Superseded (skip):** Model fallback unification (superseded by eval_scaffold), paste cursor fix (already on main as 47dca25), animation toggle (keybinding conflict with archive browser on 'A').

**Recommendation:** Cherry-pick selectively in priority order. The streaming + live tokens features (c6c1a6f + 4110ca3) may be easier to reimplement on current main given API divergence.

---

### PRIORITY 4 — Cherry-pick/Reimplement: tui-pink-lifecycle

**Branch:** tui-pink-lifecycle (2 commits, 10 files changed)
**Both commits are valuable with no equivalent on main:**
1. **d9ae9ee** — Structured lifecycle phase annotations with click-to-navigate (PhaseAnnotation, ActivePhase enum, per-phase animation colors)
2. **e1df443** — Progressive rendering of coordinator responses (streaming text, tool call visibility, live detail view, frame-rate polling)

**Conflicts:** 18 across 9 files (moderate-high — mostly positional, not semantic).
**Recommendation:** Reimplement both features against current main rather than cherry-picking. Phase annotations first (smaller), then progressive rendering (larger but high UX value). The coordinator_agent.rs streaming code must be adapted to main's per-coordinator API.

---

### PRIORITY 5 — Selective cherry-pick: fix-output-section

**Branch:** fix-output-section (30 commits, 171 files)
**WARNING:** Full merge would REGRESS main (delete ~47k lines — branch is far behind).

Cherry-pick candidates:
1. **1e333ee** — Before-edge normalization (DUPLICATE — same commit as fix-before-edges, handle there)
2. **10fe502** — Verify string validation + circuit breaker (DUPLICATE — also on fix-before-edges)
3. **e1429b1** — Detail view collapse toggle (DUPLICATE — also on fix-before-edges)

Unique value:
4. **11 design docs** — all new files, no conflicts, valuable reference material:
   - self-healing-task-graph.md, coordinator-as-graph-citizen.md, deep-synthesis.md (HIGH)
   - compactor-agent-spec.md, coordinator-compactor-architecture.md, temporal-navigation.md, agent-activity-protocol.md, auto-evolver-loop.md, agency-model-quality-tiers.md (MEDIUM)
   - archive-review-human-interaction.md, integration-roadmap.md (LOW)

**Superseded:** Model fallback unification, gitignore/Cargo.lock, dot-task visibility fixes (state no longer exists), instant dot-task toggle (AnimationMode::ReducedMotion), fade-out animation, rainbow spectral tab, paste off-by-one.

**Recommendation:** Cherry-pick the 11 design docs (clean file additions). All code either duplicates fix-before-edges commits or is superseded. **Abandon the branch after extracting docs.**

---

### PRIORITY 6 — Selective cherry-pick: fix-auto-task-edges

**Branch:** fix-auto-task-edges (4 commits, 8 files)

Valuable:
1. **abcfece** — Related Tasks TUI section (FULLY valuable — new feature)
2. **02391e1** — Bidirectional edge fix for auto-created system tasks (PARTIALLY valuable — eval side useful, verify wiring approach differs from main)
3. **0b13a2b** — Caller identity logging on service stop (PARTIALLY valuable — logging useful, restart command may conflict)

Superseded:
4. **a58698b** — Chat transcript parsing (mostly superseded by main's `extract_assistant_text_from_log`)

**Conflicts:** 5 files (coordinator.rs, mod.rs, main.rs, event.rs, state.rs).
**Recommendation:** Cherry-pick abcfece (Related Tasks section) and the useful parts of 02391e1 and 0b13a2b. Skip a58698b.

---

## Section 2: Branches to Abandon (Fully Superseded)

### infra-fix-toctou — ABANDON

**Reason:** Its only unique commit (de17801 — the TOCTOU fix) is superseded by fix-toctou-race, which has a phased implementation plus additional safety infrastructure. fix-toctou-race also includes the `GraphLock` guard pattern concept. 49 merge conflicts make cherry-picking nonviable.

**Action:** Delete branch. Use fix-toctou-race as the vehicle for the TOCTOU fix.

### provenance-and-executor-generalization — ABANDON

**Reason:** All 3 commits (~5,800 lines of code) are independently implemented on main. trace.rs, replay.rs, runs_cmd.rs, provenance recording — all exist on main in more evolved form. Even the integration tests exist on main.

**Optional:** Cherry-pick 2 nikete review docs (docs/research/nikete-fork-deep-review.md and nikete-logging-review.md) — these are the only surviving record of nikete's fork design (repo is now 404). Purely archival.

**Action:** Delete branch after optionally extracting the 2 review docs.

---

## Section 3: Branches Needing Manual Review

None of the 11 branches require manual review beyond what is described above. All branches have clear recommendations. However, the following items deserve maintainer judgment:

1. **TOCTOU fix urgency:** The auditors flagged this as a real correctness bug with data loss under concurrent agent operation. The maintainer should decide whether to prioritize this as a hotfix or schedule it normally.

2. **Streaming/progressive rendering overlap:** Both show-live-token (via fix-before-edges) and tui-pink-lifecycle implement streaming coordinator responses. The implementations differ (file-polling vs. similar but separate). The maintainer should decide which approach to adopt or whether to merge the best ideas from both.

3. **Animation toggle keybinding:** fix-before-edges has 'A' for animation toggle, but main maps 'A' to archive browser. Needs a keybinding decision before that commit can land.

4. **Design docs volume:** fix-output-section has 11 design docs. Some describe features already implemented (compactor). The maintainer should decide which are still relevant reference material vs. historical artifacts.

---

## Section 4: External Contributions (nikete branches)

### nikete/main — Extract docs, abandon code

**Contributor:** nikete (nikete@gmail.com) at GroupLang, co-authored with Claude Opus.
**Relationship:** Good-faith research collaboration. Main already acknowledges nikete's contributions in `docs/design/vx-integration-response.md`.

**Code (all superseded):**
- Replay system (capture/distill/replay) — main has independently evolved implementations
- trace, replay, runs commands — all exist on main in more mature form
- Uses `blocked_by` field (renamed to `after` on main)

**Valuable novel contribution — Canon/Distillation system:**
- `src/canon.rs` (627 LOC) — Distilled knowledge artifacts, versioning, prompt rendering
- `src/commands/distill.rs` + `canon_cmd.rs` — CLI for distillation pipeline
- Stream-JSON trace parser in `src/trace.rs` — conversation-level parsing (complementary to main's provenance-based trace)
- This fills a genuine gap: main has trace + replay but no distillation layer

**Valuable docs (no equivalent on main):**
- docs/design-replay-system.md — foundational design with 6 tradeoff analyses
- docs/design-veracity-exchange.md (737 lines) — proper scoring rules, attribution methods, sensitivity classification

**Recommendation:**
1. Port the canon/distillation system concept as a new feature (adapt to current API)
2. Consider porting the stream-JSON trace parser as a companion to main's trace
3. Preserve the 2 design docs as reference material
4. Credit nikete in any resulting PRs

### nikete/vx-adapter — Extract docs, abandon code

**Code (all superseded):** Wholesale agency→identity/motivation→objective/evaluation→reward renames across 87 files. Main kept its own terminology and restructured into a directory module. 390-commit divergence makes code merge impossible.

**Valuable docs (cherry-pick with terminology updates):**
1. **docs/research/organizational-economics-review.md** (765 lines) — Maps 13 foundational economics results to wg design. Identifies concrete design risks (Goodhart, underexploration, tournament pathologies). Highest intellectual value.
2. **docs/research/collaborators-and-perspectives.md** (356 lines) — Team context document
3. **docs/research/gepa-integration.md** (426 lines) — 10 concrete GEPA integration points

**Recommendation:** Cherry-pick all 3 research docs with s/Objective/Tradeoff/, s/Reward/Evaluation/, s/Identity/Agency/ terminology updates. Preserve FORK.md as `docs/archive/nikete-fork-rationale.md`.

---

## Section 5: Recommended Action Plan

Ordered steps for the maintainer, from highest impact to lowest:

### Phase 1 — Critical Fix (do first)
1. **Land TOCTOU fix** from fix-toctou-race
   - Cherry-pick b3ada7a (core mutate_graph), 39e1651 (command conversions), 442556e (remaining sites)
   - This fixes a real data-loss bug under concurrent agent operation
   - Expect significant conflict resolution (~39 files)
   - Consider reimplementing against current main if conflicts are too severe

### Phase 2 — Quick Wins (small, high-value, low-conflict)
2. **Merge tui-disable-fade** (43b3c7a) — 1 trivial conflict, net -108 lines
3. **Cherry-pick before-edge normalization** (1e333ee from fix-before-edges) — real bug fix
4. **Cherry-pick design docs** from fix-output-section (11 new files, zero conflicts)

### Phase 3 — High-Value Features (require rework)
5. **Cherry-pick default_agent assignment** (ffa1e54 from fix-before-edges) — self-contained
6. **Cherry-pick config panel tests** (000b4cb from fix-before-edges) — new file, minimal conflict
7. **Cherry-pick Related Tasks TUI section** (abcfece from fix-auto-task-edges)
8. **Reimplement streaming coordinator response** (from tui-pink-lifecycle or show-live-token) against current main's per-coordinator API
9. **Reimplement lifecycle phase annotations** (from tui-pink-lifecycle) against current main

### Phase 4 — Safety Infrastructure (from fix-toctou-race, after TOCTOU core lands)
10. **Cherry-pick safety commands:** wg retract, cascade-stop, hold/unhold, liveness, ask
11. **Cherry-pick self-healing remediation** for failed tasks
12. **Cherry-pick verify task split** (verify-build vs verify-test)

### Phase 5 — External Contributions & Docs
13. **Cherry-pick nikete research docs** (5 docs total across nikete/main and nikete/vx-adapter) with terminology updates
14. **Cherry-pick nikete review docs** from provenance branch (2 archival docs)
15. **Port canon/distillation system concept** from nikete/main as a new feature task

### Phase 6 — Cleanup
16. **Delete branches:** infra-fix-toctou, provenance-and-executor-generalization
17. **Archive branches after extraction:** fix-output-section, fix-auto-task-edges, fix-before-edges (subsumes show-live-token)
18. **Keep for reference until reimplemented:** fix-toctou-race, tui-pink-lifecycle
19. **External branches:** No action needed (they're on nikete's remote)

---

## Appendix: Branch-by-Branch Quick Reference

| Branch | Verdict | Unique Commits | Valuable | Superseded | Conflicts | Action |
|--------|---------|---------------|----------|------------|-----------|--------|
| fix-toctou-race | CHERRY-PICK (phased) | 59 | 51 | 3 (+5 merges) | 39 files | Phase 1+4 |
| tui-disable-fade | MERGE | 1 | 1 | 0 | 1 trivial | Phase 2 |
| fix-before-edges | CHERRY-PICK | 11 | 8 | 1 full + 2 partial | 7 files | Phase 2+3 |
| show-live-token | SUBSUMED | 10 | — | — | — | Handled via fix-before-edges |
| tui-pink-lifecycle | REIMPLEMENT | 2 | 2 | 0 | 18 (9 files) | Phase 3 |
| fix-output-section | EXTRACT DOCS | 30 | 7 code + 11 docs | 12 | 9 files | Phase 2 (docs), abandon code |
| fix-auto-task-edges | CHERRY-PICK | 4 | 1 full + 2 partial | 1 | 5 files | Phase 3 |
| infra-fix-toctou | ABANDON | 1 unique | 0 (superseded by fix-toctou-race) | 1 | 49 files | Phase 6 |
| provenance | ABANDON | 3 | 0 code + 2 archival docs | all code | none | Phase 5+6 |
| nikete/main | EXTRACT | 3 | canon system + 2 docs | replay/runs/trace code | N/A | Phase 5 |
| nikete/vx-adapter | EXTRACT DOCS | 5 | 3 research docs | all code (87-file rename) | N/A | Phase 5 |

---

*Report generated by synthesizing reports from 11 independent audit agents. Each branch was audited commit-by-commit against current main with conflict analysis.*
