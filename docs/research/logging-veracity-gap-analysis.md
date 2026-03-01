# Logging/Provenance vs. Veracity Exchange: Gap Analysis

**Author**: scout (analyst)
**Date**: 2026-02-19
**Task**: eval-logging-vs

---

## Sources Reviewed

- `docs/design/provenance-system.pdf` — our provenance system design (7 pages)
- `docs/research/nikete-logging-review.pdf` — review of nikete's fork replay system (8 pages)
- `docs/LOGGING.md` — current logging documentation
- `src/provenance.rs` — operation log implementation (327 lines)
- `tests/integration_logging.rs` — 29 integration tests for logging
- `docs/AGENCY.md` — agency system with evaluation/evolution
- `docs/research/amplifier-integration-proposal.md` — executor model analysis
- `research-veracity-exchange` task logs — summary of the veracity exchange integration research
- `compare-nikete-vs` task (abandoned) — decision to build on our architecture
- `integrate-logging-system` task artifacts — all 12 commands now instrumented

---

## What the Veracity Exchange Vision Requires

Nikete's veracity exchange concept (from the research-veracity-exchange task) proposes:

1. **Outcome-scored sub-units**: Each task has measurable real-world outcomes (portfolio P&L, prediction MSE, negative mean squared error). These become veracity scores.
2. **Trust market**: Public (non-sensitive) prompt sections posted to a market where others suggest improvements. Good suggestions signal credibility.
3. **Network learning**: Learn which peers to do veracity exchanges with — trust built on demonstrated competence.
4. **Information flow control**: The graph structure controls what data leaves your node. Task interfaces define public/private boundaries.
5. **Latent payoff**: Some sub-units have outcomes that take time to evaluate (e.g., portfolio performance over weeks).

---

## What the Current System Provides

### Operation Log (fully implemented)
- Append-only JSONL at `.workgraph/log/operations.jsonl`
- All 12 graph-mutating commands instrumented: add, edit, done, fail, abandon, retry, claim, unclaim, pause, resume, archive, gc
- Each entry: `{timestamp, op, task_id, actor, detail}`
- Actor convention: `"cli"`, `"agent:<id>"`, `"coordinator"`
- Zstd-compressed rotation at configurable threshold (default 10 MB)
- `read_all_operations()` reads across rotated + current files transparently
- Concurrent write safety via `O_APPEND` + single `write_all()` call

### Agent Conversation Archives (implemented)
- On `wg done` or `wg fail`: agent's `prompt.txt` and `output.log` archived to `.workgraph/log/agents/<task-id>/<timestamp>/`
- Each retry attempt gets its own timestamped subdirectory
- Full history of all attempts preserved permanently

### Agency Evaluation System (implemented)
- 4-dimension scoring: correctness (40%), completeness (30%), efficiency (15%), style_adherence (15%)
- Evaluations stored in `.workgraph/agency/evaluations/` as YAML
- Scores propagate to agent, role, and motivation performance records
- Performance trends computed (up/down/flat)
- Synergy matrix (role × motivation cross-performance)

### Agency Evolution (implemented)
- Mutation, crossover, gap-analysis, retirement, motivation-tuning strategies
- LLM-powered evolver agent proposes structured operations
- Content-hash IDs ensure identity immutability
- Lineage tracking (parent_ids, generation, created_by)

### Coherency Checks (implemented)
- Integration tests verify operation log matches graph state
- Every task in graph has corresponding `add_task` entry
- Terminal states (done/failed/abandoned) have corresponding log entries
- Archived/gc'd tasks verified removed from graph

---

## Dimension Scores

### 1. Outcome Tracking — Score: 2/5

**What supports veracity exchange:**
- The evaluation system scores task outputs on a 0–1 scale across 4 dimensions
- Evaluations are stored per-task with timestamps, agent context, and model metadata
- Performance records aggregate scores at agent, role, and motivation levels
- The `detail` field in `OperationEntry` accepts arbitrary JSON, so outcome data *could* be attached to log entries today with no schema change

**What's missing:**
- **No domain-specific outcome scores.** Evaluations are LLM-judged quality assessments, not real-world outcomes (P&L, MSE, prediction accuracy). The veracity exchange needs ground-truth metrics, not proxy quality scores.
- **No latent outcome mechanism.** There's no way to attach an outcome to a task weeks after completion. Evaluations happen once, at completion time. Veracity exchange explicitly requires deferred scoring.
- **No outcome taxonomy.** The system has one score type (evaluation). Veracity exchange needs multiple score types: quality (current), financial return, prediction accuracy, peer review, etc.
- **No score update/revision.** Once an evaluation is written, it's immutable. Latent payoffs require updating scores over time.

**Gap to close:** Add an `outcome` field to tasks or a separate outcomes store that supports: (a) multiple score types with arbitrary keys, (b) deferred attachment (score added long after task completion), (c) score revision/update. The evaluation system provides the infrastructure pattern — the gap is semantic, not architectural. Estimated: ~200 lines for a basic `wg outcome add <task-id> --type pnl --value 0.12` command + storage.

### 2. Audit Completeness — Score: 4/5

**What supports veracity exchange:**
- **Full mutation history.** Every graph-mutating command recorded with who/when/what. This is the strongest foundation for veracity exchange — you can prove exactly what happened.
- **Actor attribution.** Every operation tagged with `"cli"`, `"agent:<id>"`, or `"coordinator"`. Can answer "who did this work?"
- **Agent conversation archives.** Full prompts and outputs preserved per attempt. Can prove what an agent was asked and what it produced.
- **Deterministic reconstruction.** The operation log is complete enough to rebuild graph state at any timestamp (designed in provenance system, forward-replay algorithm specified).
- **Content-hash agent IDs.** Agency entities are identified by SHA-256 of their identity-defining fields. Tamper-evident.
- **29 integration tests** verify logging completeness and coherency.

**What's missing:**
- **No structured conversation traces.** Agent output is raw text, not parsed into turns/tool-calls/results. nikete's `TraceEvent` enum (System, Assistant, ToolResult, User, Error, Outcome) solves this — the fork review recommends adopting it.
- **No artifact content hashing.** `task.artifacts` stores file paths only. The provenance design *proposes* SHA-256 hashing at registration time (Section 3), but it's not yet implemented. Without this, you can't prove an artifact wasn't modified after registration.
- **No LLM token/cost tracking.** Can't prove how much compute was used per task. Requires parsing `stream-json` output — identified as future work.
- **No consumed-artifact traceability.** The provenance design proposes recording which artifact hashes a downstream task consumed (Section 3, Traceability), but this isn't implemented. Critical for proving information provenance chains.

**Gap to close:** (a) Adopt nikete's `TraceEvent` + `parse_stream_json()` for structured traces. (b) Implement artifact content hashing from provenance design Section 3. (c) Record consumed artifacts at spawn time. These are all designed — just not built yet. Estimated: ~500 lines total across 3 PRs.

### 3. Information Boundary Control — Score: 1/5

**What supports veracity exchange:**
- **Graph structure as implicit boundary.** Dependency edges control information flow — a task only receives context from its direct dependencies. This is the foundation the veracity exchange research identified (the graph as information flow controller).
- **Artifact system as interface.** Tasks communicate through declared artifacts, not shared state. This creates natural interfaces between tasks.
- **Agent directory isolation.** Each agent runs in its own directory with its own prompt/output.

**What's missing:**
- **No visibility classification.** Tasks, artifacts, and log entries have no public/private/restricted marking. Everything is uniformly internal.
- **No per-task access control.** Any agent or CLI user can read any task's data.
- **No artifact visibility tagging.** Can't mark an artifact as "safe to share externally" vs "internal only."
- **No prompt section classification.** The veracity exchange specifically envisions public prompt sections (shareable for improvement suggestions) vs private sections. No mechanism for this.
- **No data export controls.** No way to selectively expose task results to external consumers while withholding internal details.
- **No concept of "external consumer" at all.**

**Gap to close:** This is the largest gap and requires new data model work:
1. Add `visibility: public | private | restricted` to Task (~20 lines in `graph.rs`)
2. Add `visibility` to artifacts (~10 lines)
3. Add a `wg export` command that respects visibility when generating output for external consumption (~100 lines)
4. Add visibility filtering to operation log queries (~30 lines)

The veracity exchange research estimated ~300-400 lines of core changes for outcome scoring + visibility. This is the visibility half. The graph-as-boundary concept is architecturally present but has no enforcement mechanism.

### 4. Replay/Reproducibility — Score: 2/5

**What supports veracity exchange:**
- **Operation log supports forward replay.** The provenance design (Section 5) specifies a reconstruction algorithm: read entries up to time T, apply in order to rebuild graph state. The data is sufficient.
- **Agent prompts archived per attempt.** Can re-send the same prompt to a different model.
- **Operation log captures before/after state.** Edit entries record `{field, old, new}` diffs. Claim entries record `prev_status`.
- **Actor field enables per-model analysis.** Can filter operations by agent to compare model performance.

**What's missing:**
- **No replay mechanism.** The `wg replay <timestamp>` command is explicitly deferred ("future — not MVP" in provenance design Section 6). The reconstruction algorithm is specified but not implemented.
- **No snapshot/restore.** nikete's fork has `snapshot()` + `restore_run()` with run IDs, recursive directory copy, and selective task reset. Our system has nothing equivalent.
- **No selective re-execution.** nikete's `--failed-only`, `--below-score`, `--tasks`, `--keep-done` options for targeted replay don't exist in our system.
- **No run management.** No concept of "run IDs" or the ability to compare executions across model versions.
- **No structured traces for replay enrichment.** nikete's canon system (distill conversations into reusable knowledge) depends on structured traces, which we don't have.

**Gap to close:** The nikete fork review recommends adopting the replay system with modifications:
1. Adopt `snapshot()` + `restore_run()` (~200 lines from `runs.rs`)
2. Adopt selective task reset (`reset_tasks_for_replay()`, ~100 lines)
3. Add `wg runs list/show/restore` commands (~200 lines)
4. Wire replay operations into the provenance log (new: replay ops aren't logged in nikete's fork)
5. Adopt `wg replay --model <model>` with filtering options (~400 lines from `commands/replay.rs`)

This is the largest implementation gap after information boundary control. nikete's fork provides well-tested reference code (52 tests).

### 5. Exchange Protocol Readiness — Score: 1/5

**What supports veracity exchange:**
- **`wg log --operations --json`** provides machine-readable operation history. An external consumer could parse this.
- **Evaluation scores exist in structured YAML.** A consumer could read `.workgraph/agency/evaluations/`.
- **Artifact paths are declared per task.** A consumer knows what files a task produced.
- **Content-hash IDs provide stable references.** Agents and tasks can be referenced unambiguously.

**What's missing:**
- **No exchange protocol.** No defined format for sharing task results, no transport mechanism, no API.
- **No external consumer concept.** The system is entirely local — no authentication, no access control, no concept of "peers."
- **No credibility tracking.** The veracity exchange builds trust through demonstrated competence. No mechanism to track external peer credibility.
- **No selective export.** Can't expose specific task results while withholding others.
- **No discovery mechanism.** No way for external nodes to discover available task results.
- **No suggestion/improvement interface.** The trust market concept requires accepting improvement suggestions from external peers.

**Gap to close:** The veracity exchange research recommended a 3-phase approach:
1. **Phase 1 (CLI bridge)**: `wg exchange export <task-id>` generates a portable JSON bundle (task definition, evaluation scores, public artifacts). `wg exchange import` consumes bundles from peers. ~300-400 lines.
2. **Phase 2 (Event hooks)**: Hook system that notifies external systems on task completion. Allows passive consumers to subscribe. ~200 lines.
3. **Phase 3 (Native integration)**: Full exchange client with peer discovery, credibility tracking, and bidirectional communication. This is a separate tool, not core workgraph changes.

This is correctly deferred as the most speculative dimension. Phase 1 alone provides the minimum viable exchange interface.

---

## Score Summary

| Dimension | Score | Assessment |
|-----------|-------|------------|
| **Outcome Tracking** | 2/5 | Evaluation infrastructure exists but tracks LLM quality, not real-world outcomes. No latent payoff mechanism. |
| **Audit Completeness** | 4/5 | Strongest dimension. Full mutation history, actor attribution, agent archives, 29 tests. Missing: structured traces, artifact hashing. |
| **Information Boundary Control** | 1/5 | Largest gap. No visibility classification at any level. Graph provides implicit boundaries but nothing is enforceable. |
| **Replay/Reproducibility** | 2/5 | Data exists for replay but no mechanism. nikete's fork has reference implementation. |
| **Exchange Protocol Readiness** | 1/5 | Essentially unstarted. Machine-readable logs exist but no protocol, no peers, no export. |

**Weighted average: 2.0/5** (equal weights)

---

## Priority Ranking for Closing Gaps

### Tier 1: Foundation (enables Tiers 2-3)

1. **Structured conversation traces** — Adopt nikete's `TraceEvent` + `parse_stream_json()`. Required for replay enrichment, audit depth, and eventually distillation. High value for internal use regardless of veracity exchange.
2. **Artifact content hashing** — Implement SHA-256 hashing from provenance design Section 3. Required for proving artifact integrity in any exchange scenario. Already designed.

### Tier 2: Core Veracity Capabilities

3. **Outcome scoring** — Add `wg outcome` command for domain-specific, deferrable scores. Extends evaluation infrastructure with real-world metrics. Required for veracity exchange's fundamental premise.
4. **Visibility classification** — Add `visibility` field to tasks and artifacts. Required for information boundary control. Small schema change with large implications.
5. **Replay mechanism** — Adopt nikete's snapshot/reset/re-execute with provenance integration. Enables model comparison and iterative improvement.

### Tier 3: Exchange Infrastructure (build when needed)

6. **Export format** — Define portable JSON bundle for sharing task results. First step toward exchange protocol.
7. **Credibility tracking** — Track peer reliability scores. Extends the agency evaluation pattern to external participants.
8. **Exchange protocol** — Full peer-to-peer communication. Deferred until the use case crystallizes.

---

## Key Architectural Observation

The current logging/provenance system is built as an **internal audit trail**. Veracity exchange requires it to also serve as an **external credibility signal**. The gap is not primarily technical — the infrastructure (append-only log, evaluation scores, agent archives) is solid. The gap is **semantic**: the system records *what happened* but not *what it's worth* (outcome scoring), *who can see it* (visibility), or *how to share it* (exchange protocol).

The agency system's evaluate/evolve loop is the closest existing analog to veracity exchange. Evaluations score outputs, evolution improves agents based on scores — this is a closed-loop version of what veracity exchange does across organizational boundaries. Extending evaluations with real-world outcome metrics and adding visibility controls would bridge the internal agency loop to the external veracity exchange with minimal architectural change.

The operation log's `detail: serde_json::Value` field is intentionally open-ended and already handles arbitrary payloads. Outcome scores, visibility metadata, and exchange identifiers can all be attached via the detail field without schema changes to `OperationEntry`. This is a deliberate design choice that pays off here — the log format is already exchange-ready even if the tooling isn't.
