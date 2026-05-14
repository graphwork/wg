# Document Review: Terminal-Bench & Executor Docs

**Task:** research-review-new
**Date:** 2026-04-03

> **Model update note**: This review was written when the experiment planned to use Qwen3-32B. The primary model was subsequently changed to **Minimax M2.7** because Qwen3-32B was expected to score near 0%, giving no useful signal. References to "Ollama → OpenRouter" changes and Qwen3-specific content below are historical.

---

## Group 1: Terminal-Bench Docs (To Be Committed)

### 1. ROADMAP-terminal-bench.md

**Summary:** A phased 8-11 day plan to validate the wg thesis via Terminal Bench. Starts with smoke-testing the native executor with Claude and Qwen3-32B (Phase 0), proceeds through fixing critical bugs — silent JSON parse failures, hardcoded 200K context budget, Qwen3 tool call format support (Phase 1), adds observability (streaming, heartbeat, graceful context exhaustion — Phase 2), builds the Terminal Bench harness for both conditions A (bare agent) and B (agent + wg — Phase 3), runs experiments (Phase 4), and writes up results (Phase 5). Includes risk assessment and a "nuclear option" fallback to Claude Haiku if open models struggle.

**Changes vs existing version:** All changes are Ollama → OpenRouter substitutions:
- "Qwen3/Ollama" → "Qwen3 via OpenRouter" (current state assessment)
- Smoke test commands: `ollama pull` + localhost curl → OpenRouter curl with auth headers
- Context window config: "Query Ollama `/api/show`" → "Look up from OpenRouter model metadata or config"
- Tool call validation: "check if Ollama's endpoint..." → "check if OpenRouter's endpoint..."
- Integration test: "Starts Ollama" → "Connects to Qwen3-32B via OpenRouter"
- Phase 4 headings: "via Ollama" → "via OpenRouter"
- Stretch model: "Qwen3-8B via Ollama" → "Smaller open model via OpenRouter (e.g., Qwen3-8B, DeepSeek-V3)"
- Risk row: "Ollama is too slow for full benchmark (50%)" → "OpenRouter rate limits or downtime (20%)" — notably the probability dropped from 50% to 20%, reflecting that API-based inference is more reliable than local

**Claude Code references:** None. Clean.

**Ollama references:** None remaining after the update. Clean.

---

### 2. REFERENCE-terminal-bench-campaign.md

**Summary:** The comprehensive knowledge base for the Terminal Bench campaign. Covers the thesis (memory makes computation universal, arXiv:2412.17794), Terminal Bench overview (89 tasks, Docker containers, outcome-based tests), detailed ForgeCode competitive analysis (how they went 25% → 81.8%, with the critical insight that enforced planning was +28 points), architectural comparison (ForgeCode vs wg), native executor state and bugs, full experiment design (Conditions A/B/C, metrics, expected results), Terminal Bench 2.0 integration approach (Harbor framework, Docker injection, adapter pseudocode, submission process), timeline, testing requirements, and strategic context. This is the single most important campaign document — it's both briefing and playbook.

**Changes vs existing version:**
- Bug table: "Query Ollama `/api/show`" → "Look up from OpenRouter model metadata or config"; "check if Ollama returns native `tool_calls`" → "check if OpenRouter returns..."
- Model section: "Qwen3-32B via Ollama (or vLLM for throughput) / Free, local, no API costs" → "Qwen3-32B via OpenRouter / Cheap (~$0.20/MTok input, ~$0.60/MTok output) / No local GPU needed"
- Calibration model: added "(Anthropic API)" clarifier
- Pre-build path: `/home/erik/executors/wg` → `<wg-repo-root>` (sanitized)
- Metadata YAML: `model_provider: ollama` → `model_provider: openrouter`
- Time estimates: "With Ollama local: slower (single GPU inference)" → "With smaller/slower models: proportionally longer"
- Smoke test: "Qwen3-32B via Ollama" → "Qwen3-32B via OpenRouter"
- Setup commands: `/home/erik/executors/wg` → `<wg-repo-root>` (sanitized)
- Links section: Removed absolute paths to `/home/erik/executors/REPORT-*.md` files; replaced with relative repo paths to `docs/terminal-bench/*.md` and "separate comparative report (not included in this repo)"

**Claude Code references (4 occurrences, all clean):**
1. Leaderboard table entry: "Claude Code | Claude Opus 4.6 | 58.0%" — factual benchmark data
2. "Beat Claude Code's 58% with an open model" — target-setting, factual
3. "Claude Code (Reference Implementation)" section header — link categorization
4. "Claude Code scores 58% on Terminal Bench 2.0" — factual

All are factual references to Claude Code as a benchmark competitor. None reveal internal architecture, reference the Claude Code source, or contain anything problematic to commit.

**Ollama references (1 remaining):**
- Line 675: `- Ollama: https://ollama.com/` in the Links Index, Models section. This is just a link listing alongside OpenRouter and vLLM. Not problematic — Ollama remains a valid model inference option.

---

### 3. DESIGN-native-executor-improvements.md

**Summary:** Eight design patterns the native executor needs for Terminal Bench readiness: (1) mid-turn state injection (ephemeral context-update blocks for messages/graph changes/context pressure), (2) tiered context pressure management (80% warning → 90% emergency compaction → 95% clean exit → API error recovery), (3) smart tool result truncation (head+tail with per-tool limits), (4) error recovery with withholding (handle 413/429/500 transparently), (5) file state cache (LRU, 100 entries, 25MB), (6) tool execution parallelism (partition read-only vs mutating), (7) session summary extraction (periodic cheap model calls), (8) prompt cache optimization (Anthropic `cache_control`). Includes a priority table: context pressure and truncation are Day 1, error recovery Day 1-2, mid-turn injection Day 2, file cache and parallelism Day 3, summary and prompt cache Day 4+.

**Changes vs existing version:** Identical. No diff.

**Claude Code references:** None. Clean.

**Ollama references:** One benign mention at line 292: "For OpenAI-compatible APIs (OpenRouter, Ollama), server-side caching is automatic." This is a technical factual statement, not problematic.

---

## Group 2: Executor Docs (Learning Only, NOT to Commit)

### 4. REPORT-claude-code-vs-wg-architecture.md

**Summary:** A deep architectural comparison of Claude Code TS and WG as multi-agent orchestration systems. Covers architecture (conversation-first vs graph-first), executor comparison (Claude Code's single in-process QueryEngine vs WG's four executor types), task/dependency systems (3-state flat model vs 60-field graph with cycles), coordinator patterns (conversation mode vs daemon tick loop), TUI/rendering approaches, embedding opportunities, messaging models, and concrete recommendations for each system. Key finding: each has critical capabilities the other lacks.

**Feedback:**

**Valuable insights:**
- The "conversation-first vs graph-first" framing is excellent — it precisely captures the fundamental architectural difference and why each system makes the tradeoffs it does.
- The gap analysis tables (Section 3.3) are actionable. The native executor's missing capabilities are clearly prioritized.
- Section 7.3 ("What WG's Native Executor Needs to Replace the Claude Executor") is the most operationally useful part — the 93.8% tool call coverage figure and the 16-25 hour gap estimate are concrete planning inputs.
- The coordinator comparison (Section 5) highlights a real weakness: WG's coordinator doesn't synthesize. It dispatches and monitors but doesn't reason about task content. This is worth thinking about long-term.

**Patterns that could inform wg development:**
- **System-reminder injection pattern**: Claude Code's ephemeral per-turn context injection (Section 9.1 reference, detailed more in REFERENCE-executor-lessons.md) is exactly the pattern described in DESIGN doc #1. Validating that a production system at scale uses this approach increases confidence in the design.
- **Progressive rendering**: Claude Code's collapsed summaries for repetitive tool calls ("Read 15 files" instead of 15 entries) would improve the TUI experience.
- **Permission model**: Not urgent, but as WG agents become more autonomous, some form of permission gating for destructive operations (especially in native executor) would be prudent. Worth a future task.

**Concerns/disagreements:**
- The recommendation to "embed Claude Code's QueryEngine directly" (Section 7.2A) is correctly assessed as low feasibility, but it's borderline distracting to even include. The native executor path is clearly the right one.
- The "in-process sub-agents" recommendation for wg (Section 8.2, point 6) is interesting but conflicts with WG's isolation model. Each agent gets its own worktree precisely to prevent file conflicts. In-process sub-agents sharing a working tree would need careful scoping to read-only tasks.

---

### 5. REPORT-effort-and-valuation.md

**Summary:** Compares the development economics of Claude Code (512K LOC TypeScript, ~20-40 engineers, ~18-24 months, est. $15-25M) vs wg (216K LOC Rust, 1 person, 75 days, est. $100-150K actual / $2-3M traditional equivalent). Analyzes the 15-20x cost efficiency gain from AI-augmented development, breaks it down by work type (boilerplate 50-100x, architecture 1-2x), provides valuations (Claude Code $2-5B standalone; wg $2-5M today, $50-150M at Series A if category creates), and identifies the core gap: the native executor needs 2-3 days to achieve independence from Claude Code.

**Feedback:**

**Valuable insights:**
- The AI multiplier breakdown by work type (Section 3) is genuinely novel and rings true. The observation that "architectural novelty is still fundamentally human-paced" while "implementation gap collapses to near-zero" matches the wg development pattern visible in the commit history.
- The "Cathedral vs Forge" metaphor (Section 4) — team-optimized vs mind-optimized codebase organization — is a useful lens. The flat 16-directory structure isn't a deficiency; it's an intentional design for single-person comprehension.
- The 29% churn rate observation as evidence of "real refactoring, not AI slop" is a sharp diagnostic.
- The valuation asymmetry discussion (Section 5) is honest: distribution and trust matter more than technical superiority for market value.

**Patterns for wg development:**
- The "2-3 days to independence" estimate for native executor gap closure is consistent across this doc and the architecture report. This convergence suggests it's a reliable estimate, and it's now partially realized given recent commits (exec-heartbeat, exec-session-summary, exec-file-cache, exec-tool-truncation).
- The meta-observation that "the 244 design documents are the real product" is worth internalizing. The docs encode the decisions; the code is output. This validates the heavy documentation investment.

**Concerns:**
- The valuations are speculative and optimistic, which is fine for internal motivation docs, but they shouldn't leak into any public-facing content. The $2-5B Claude Code figure is debatable; the wg Series A range assumes category creation that hasn't been validated yet.
- "This system built itself" is a powerful narrative but needs careful qualification in public use. The system orchestrated AI agents that wrote code under human direction. The distinction matters for credibility.

---

### 6. REFERENCE-executor-lessons.md

**Summary:** Extracts 8 specific mechanisms from Claude Code's TypeScript executor that the native executor should adopt, with implementation sketches: (1) mid-turn state injection (system-reminder + submit-interrupt patterns), (2) tiered context pressure (4-level cascade from snip to autocompact, thresholds at 87/93/97%), (3) tool result budget (per-message token limits, three-partition tracking), (4) error recovery with withholding (model never sees 413 errors), (5) file state cache (LRU, 100 entries, 25MB, mtime-based staleness), (6) session memory extraction (periodic subagent summarization), (7) prompt cache optimization (cache_control markers), (8) tool execution parallelism (partitioned concurrent/serial). Concludes with Erik's "pause-and-inject" insight mapped to Claude Code's mechanisms.

**Feedback:**

**Valuable insights:**
- This is the most directly actionable of the three Group 2 docs. Each section maps a Claude Code mechanism to a concrete native executor implementation. The DESIGN doc (Group 1, #3) appears to be a cleaned-up derivative of this document.
- The three-threshold comparison is instructive: Claude Code uses 87/93/97% while the DESIGN doc proposes 80/90/95%. The more conservative thresholds in the DESIGN doc make sense for open models with less predictable token estimation.
- The "error withholding" pattern (Section 4) is the most important non-obvious insight: the model should never see infrastructure errors. The agent loop should silently recover (compact, backoff, retry) and only surface errors that genuinely can't be handled.
- The CacheSafeParams detail (Section 7) — that forked agents reuse system prompt params for prompt cache hits — is subtle but important for cost optimization at scale.

**Patterns for wg development:**
- The priority table at the end is the best single-page engineering roadmap for native executor work. Context pressure and smart truncation are correctly identified as Day 1 priorities.
- The "pause-and-inject" framing at the end ties everything together: the agent's view of the world is always fresh because injections are ephemeral. This is the key architectural principle.

**Concerns:**
- This doc contains the most Claude Code implementation detail (specific file names, line numbers, function names). It's correctly marked as "not to commit," and indeed it should stay private. The detailed source references (`query.ts:847-862`, `toolResultStorage.ts`, etc.) could be seen as derived from proprietary analysis.
- The 10-item concurrency max from Claude Code (Section 8) is presented without discussion of whether it's optimal. For WG's use case (Rust, lighter-weight tools), a different default might be appropriate.

---

## Cross-Cutting Analysis

### Ollama → OpenRouter Shift

The Group 1 docs systematically replace Ollama (local inference) with OpenRouter (API-based inference). This is a strategic improvement:

| Dimension | Ollama (old) | OpenRouter (new) |
|-----------|-------------|-----------------|
| GPU requirement | Yes (significant VRAM for 32B) | None |
| Setup complexity | Install Ollama, pull model, manage VRAM | Set API key |
| Reproducibility | Hardware-dependent | Consistent across any machine |
| Cost | Free (hardware amortized) | ~$0.20-0.60/MTok |
| Speed | GPU-dependent | API-speed, consistent |
| Risk (old doc) | "Ollama too slow" at 50% probability | "Rate limits/downtime" at 20% |
| Accessibility | Requires beefy hardware | Laptop + internet |

The shift makes the campaign more accessible and reproducible. The cost is real but modest (~$10-30 for a full 89-task x 5-trial run). The old risk of "Ollama is too slow for the full benchmark" was the single highest-probability risk at 50%; the new OpenRouter risk drops to 20%.

One consideration: the old docs offered a local-inference-only story (no API keys, no cloud dependency). That narrative ("zero cost to run") is weakened. For the Terminal Bench campaign specifically this is fine — you're optimizing for getting results, not for a zero-cost demo. But for the broader wg pitch ("$0 open model"), you may want to retain Ollama/vLLM as documented alternatives.

### Claude Code References in Group 1 — Clean

All Claude Code references in Group 1 docs are factual benchmark data:
- Leaderboard position (rank ~39, 58.0%)
- As a comparison target ("beat Claude Code's 58%")
- Link section header

None reference Claude Code's internal architecture, source code, or proprietary details. These are safe to commit.

### Relationship Between Documents

The 6 docs form a clear pipeline:

```
REFERENCE-executor-lessons.md (Group 2: raw analysis of Claude Code mechanisms)
    ↓ distilled into
DESIGN-native-executor-improvements.md (Group 1: sanitized design patterns)
    ↓ scheduled by
ROADMAP-terminal-bench.md (Group 1: phased plan referencing the design patterns)
    ↓ contextualized by
REFERENCE-terminal-bench-campaign.md (Group 1: full campaign knowledge base)

REPORT-claude-code-vs-wg-architecture.md (Group 2: strategic comparison)
REPORT-effort-and-valuation.md (Group 2: economics and positioning)
```

The Group 2 docs are the analytical foundation; Group 1 docs are the sanitized, committable derivatives. The sanitization was done well — no proprietary details leaked through.

### What's Already Been Implemented

Cross-referencing the DESIGN doc's priority list with recent commits:
- **exec-heartbeat**: Heartbeat signal during long-running tool execution ✓
- **exec-session-summary**: Session summary extraction for journal resume ✓
- **exec-file-cache**: FileCache wired into ReadFileTool ✓
- **exec-tool-truncation**: Smart truncation for all tools ✓

This means 4 of the 8 design patterns are already implemented. Remaining: context pressure detection, error recovery/withholding, mid-turn state injection, tool execution parallelism.
