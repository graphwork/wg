# Design: Fan-Out/Fan-In Autopoietic Evolver Architecture

> Design document for `design-evolver-fanout`
> Author: Thorough Architect agent (3ede50bb)
> Date: 2026-03-13

## Overview

Transform `wg evolve run` from a single monolithic LLM call into a fan-out/fan-in
task graph that partitions evaluation data by strategy, runs analyzers in parallel,
synthesizes results, applies operations, and optionally loops for continuous improvement.

### Current State

- `wg evolve run` builds ONE prompt (~39K chars) containing ALL evaluations (currently 1,976)
- Calls `claude --print` once, parses JSON output, applies operations sequentially
- 8 strategies (mutation, crossover, gap-analysis, retirement, motivation-tuning,
  component-mutation, randomisation, bizarre-ideation) plus coordinator-evolution
- Only 1 of 8 strategy skill docs exists (`coordinator-evolution.md`)
- No parallelism, no memory of past decisions, no self-evaluation

### Target Architecture

```
wg evolve run
  │
  ├── [threshold check: <50 evals → single-shot legacy mode]
  │
  └── [≥50 evals → task graph mode]
       │
       ▼
  .evolve-partition-{run_id}
       │
       ├──► .evolve-analyze-mutation-{run_id}
       ├──► .evolve-analyze-crossover-{run_id}
       ├──► .evolve-analyze-gap-analysis-{run_id}
       ├──► .evolve-analyze-retirement-{run_id}
       ├──► .evolve-analyze-motivation-tuning-{run_id}
       ├──► .evolve-analyze-component-mutation-{run_id}
       ├──► .evolve-analyze-randomisation-{run_id}
       ├──► .evolve-analyze-bizarre-ideation-{run_id}
       │    (all run in parallel)
       │
       └──► .evolve-synthesize-{run_id}  (--after all analyzers)
              │
              └──► .evolve-apply-{run_id}
                     │
                     └──► .evolve-evaluate-{run_id}
                            │
                            └──► [cycle back-edge to .evolve-partition if autopoietic]
```

---

## 1. Data Partitioning Logic

### 1.1 Partition Function Signature

```rust
// In src/commands/evolve/partition.rs (new file)

/// A data slice prepared for a single analyzer.
pub struct AnalyzerSlice {
    /// Which strategy this slice is for.
    pub strategy: Strategy,
    /// Pre-filtered evaluations relevant to this strategy.
    pub evaluations: Vec<Evaluation>,
    /// Roles relevant to this strategy (full objects, not just IDs).
    pub roles: Vec<Role>,
    /// Tradeoffs relevant to this strategy.
    pub tradeoffs: Vec<TradeoffConfig>,
    /// Components relevant (for component-mutation).
    pub components: Vec<RoleComponent>,
    /// Pre-computed summary statistics specific to this strategy.
    pub summary: String,
    /// Model tier recommendation for this analyzer.
    pub model_tier: ModelTier,
}

#[derive(Debug, Clone, Copy)]
pub enum ModelTier {
    /// Fast, cheap — for mechanical scans (retirement, randomisation)
    Haiku,
    /// Default — for most analysis (mutation, crossover, motivation-tuning, component-mutation)
    Sonnet,
    /// Deep reasoning — for creative/structural work (gap-analysis, bizarre-ideation)
    Opus,
}

pub fn partition_evaluations(
    evaluations: &[Evaluation],
    roles: &[Role],
    tradeoffs: &[TradeoffConfig],
    agency_dir: &Path,
    config: &Config,
) -> Vec<AnalyzerSlice> { ... }
```

### 1.2 Per-Strategy Partition Criteria

Each strategy receives only the evaluations it needs. Maximum context budget per
analyzer: **400 evaluations** (≈100K chars of eval data, leaving ~100K tokens for
prompt overhead and LLM reasoning). If a slice exceeds this, apply the truncation
strategy noted below.

#### Mutation
- **Goal**: Identify improvable roles (moderate scores, not hopeless).
- **Filter**: Roles where `0.25 ≤ avg_score ≤ 0.70` AND `task_count ≥ 3`.
- **Include**: All evaluations for matching roles, plus the roles' component details.
- **Truncation**: If >400 evals, keep the 400 most recent (by timestamp).
- **Model tier**: Sonnet.

#### Crossover
- **Goal**: Find pairs of high-performing roles with complementary strengths.
- **Filter**: Roles where `avg_score ≥ 0.55` AND `task_count ≥ 3`.
- **Include**: Evaluations for matching roles, with per-dimension breakdowns.
  Also include the synergy matrix subset for these roles.
- **Supplementary data**: For each qualifying role, include its component_ids
  and outcome_id so the LLM can identify complementary skill sets.
- **Truncation**: Keep top 20 roles by score, their most recent 20 evals each.
- **Model tier**: Sonnet.

#### Gap Analysis
- **Goal**: Identify task types not well-served by existing roles.
- **Filter**: ALL evaluations (needs the broad view), but summarized.
- **Pre-computation**: Instead of raw evals, provide:
  - Task tag/title frequency distribution (top 50 tags/patterns).
  - Per-role coverage: which task tags each role has been assigned to.
  - Roles with 0 evaluations (never deployed).
  - Average score by task tag (where inferrable from task_id patterns).
- **Include**: Role summaries (name, description, component_ids, outcome_id, avg_score)
  for ALL roles, but NOT individual evaluations.
- **Truncation**: Summary is inherently bounded; no raw eval data sent.
- **Model tier**: Opus (requires structural reasoning about coverage gaps).

#### Retirement
- **Goal**: Identify consistently poor performers with sufficient signal.
- **Filter**: Roles where `avg_score < 0.35` AND `task_count ≥ 5` (high confidence).
  Also tradeoffs where `avg_score < 0.35` AND `task_count ≥ 5`.
- **Include**: All evaluations for matching entities (to show the pattern).
- **Supplementary data**: Count of agents using each role/tradeoff (to assess
  impact of retirement).
- **Truncation**: If >400 evals, keep the 400 with lowest scores.
- **Model tier**: Haiku (mechanical scan with clear criteria).

#### Motivation Tuning
- **Goal**: Optimize tradeoff configurations based on performance correlations.
- **Filter**: All tradeoffs with `task_count ≥ 2`.
- **Include**: Synergy matrix (role × tradeoff avg scores), tradeoff details.
  Per-tradeoff dimension breakdowns if available.
- **Pre-computation**: Correlation analysis — which tradeoff parameters correlate
  with high/low scores across different role types.
- **Truncation**: Keep top 30 tradeoffs by eval count, 20 most recent evals each.
- **Model tier**: Sonnet.

#### Component Mutation
- **Goal**: Improve or replace individual role components.
- **Filter**: Components where `avg_score` is available AND `task_count ≥ 2`.
- **Include**: Component performance records with context_id (role_id) to show
  which roles benefit from which components. Resolved component content
  (actual skill text, not just hash references).
- **Pre-computation**: Component performance matrix — same component's score
  across different roles (context_ids in EvaluationRef).
- **Truncation**: Top 30 components by eval count, 15 evals each.
- **Model tier**: Sonnet.

#### Randomisation
- **Goal**: Propose random compositions from the existing primitive inventory.
- **Filter**: Minimal eval data needed — just the inventory.
- **Include**: List of all component IDs/names, outcome IDs/names,
  tradeoff IDs/names. Existing role compositions (to avoid duplicates).
  Performance summary of existing compositions (to weight toward under-explored
  combinations).
- **Truncation**: No eval data, so inherently bounded.
- **Model tier**: Haiku (combinatorial, not analytical).

#### Bizarre Ideation
- **Goal**: Generate novel, unconventional primitives.
- **Filter**: Minimal eval data — just enough context about the current
  ecosystem to inspire divergent thinking.
- **Include**: Names and descriptions of all existing components, outcomes,
  and tradeoffs (no scores — scores constrain creativity). The 5 highest-scoring
  and 5 lowest-scoring roles (to know what works and what doesn't).
- **Supplementary data**: Past bizarre_ideation operations from evolution history
  (to avoid repeating the same ideas).
- **Truncation**: Inherently bounded.
- **Model tier**: Opus (requires creativity).

### 1.3 Partition Implementation

```rust
fn partition_for_mutation(
    evaluations: &[Evaluation],
    roles: &[Role],
    tradeoffs: &[TradeoffConfig],
    max_evals: usize,  // default: 400
) -> AnalyzerSlice {
    let target_role_ids: HashSet<&str> = roles
        .iter()
        .filter(|r| {
            r.performance.task_count >= 3
                && r.performance.avg_score.map_or(false, |s| s >= 0.25 && s <= 0.70)
        })
        .map(|r| r.id.as_str())
        .collect();

    let mut filtered_evals: Vec<Evaluation> = evaluations
        .iter()
        .filter(|e| target_role_ids.contains(e.role_id.as_str()))
        .cloned()
        .collect();

    // Sort by timestamp descending, truncate
    filtered_evals.sort_by(|a, b| b.timestamp.cmp(&a.timestamp));
    filtered_evals.truncate(max_evals);

    let filtered_roles: Vec<Role> = roles
        .iter()
        .filter(|r| target_role_ids.contains(r.id.as_str()))
        .cloned()
        .collect();

    AnalyzerSlice {
        strategy: Strategy::Mutation,
        evaluations: filtered_evals,
        roles: filtered_roles,
        tradeoffs: tradeoffs.to_vec(), // need all tradeoffs for context
        components: vec![], // loaded separately if needed
        summary: String::new(), // built later
        model_tier: ModelTier::Sonnet,
    }
}
```

Similar functions for each strategy. The partition function calls all of them and
returns `Vec<AnalyzerSlice>`. Empty slices (no matching data) are skipped — the
corresponding analyzer task is not created.

### 1.4 Data Delivery Mechanism

Each analyzer receives its data as a **JSON artifact file**, not inline in the
task description. This keeps task descriptions readable and allows the data to
be larger than what fits in a wg task description.

```
.wg/evolve-runs/{run_id}/
  ├── mutation-slice.json
  ├── crossover-slice.json
  ├── gap-analysis-slice.json
  ├── retirement-slice.json
  ├── motivation-tuning-slice.json
  ├── component-mutation-slice.json
  ├── randomisation-slice.json
  └── bizarre-ideation-slice.json
```

Each slice JSON file has this schema:

```json
{
  "strategy": "mutation",
  "run_id": "run-20260313-144800",
  "timestamp": "2026-03-13T14:48:00Z",
  "evaluations": [ ... ],
  "roles": [ ... ],
  "tradeoffs": [ ... ],
  "components": [ ... ],
  "summary": "Pre-computed summary text...",
  "stats": {
    "total_evaluations_in_system": 1976,
    "evaluations_in_slice": 142,
    "roles_in_slice": 8,
    "truncated": false
  }
}
```

---

## 2. Analyzer Task Schema

### 2.1 Task Template

Each analyzer task is created with `wg add` and follows this template:

```rust
/// Create an analyzer task for a given strategy slice.
fn create_analyzer_task(
    slice: &AnalyzerSlice,
    run_id: &str,
    partition_task_id: &str,
    dir: &Path,
    config: &Config,
) -> Result<String> {
    let task_id = format!(".evolve-analyze-{}-{}", slice.strategy.label(), run_id);
    let data_path = format!(
        ".wg/evolve-runs/{}/{}-slice.json",
        run_id,
        slice.strategy.label()
    );
    let model = match slice.model_tier {
        ModelTier::Haiku => "haiku",
        ModelTier::Sonnet => "sonnet",
        ModelTier::Opus => "opus",
    };

    // Build description
    let description = format!(
        r#"## Evolver Analyzer: {strategy}

Analyze the evaluation data for the **{strategy}** evolution strategy and propose operations.

### Input
Read your data slice from: `{data_path}`

The file contains pre-filtered evaluations, roles, and tradeoffs relevant to your strategy.

### Strategy Skill Document
{skill_doc}

### Instructions
1. Read the data slice JSON file
2. Analyze the data according to the {strategy} strategy guidelines
3. Propose concrete operations (create/modify/retire/etc.)
4. Write your output as a JSON artifact

### Output Format
Write a JSON file to `.wg/evolve-runs/{run_id}/{strategy}-proposals.json`:

```json
{{
  "strategy": "{strategy}",
  "run_id": "{run_id}",
  "operations": [
    {{
      "op": "<operation_type>",
      "target_id": "<existing entity ID>",
      "rationale": "<why this operation>",
      "confidence": <0.0-1.0>,
      "expected_impact": "<what improvement is expected>",
      ... (strategy-specific fields per EvolverOperation schema)
    }}
  ],
  "analysis_summary": "<brief summary of findings>",
  "skipped_candidates": [
    {{
      "entity_id": "<ID>",
      "reason": "<why not proposed>"
    }}
  ]
}}
```

### Confidence Scoring
Rate each proposed operation's confidence:
- **0.9+**: Clear signal, high eval count, obvious action
- **0.7-0.9**: Good signal, some ambiguity
- **0.5-0.7**: Moderate signal, worth trying
- **<0.5**: Speculative, include rationale for why it's still worth proposing

## Validation
- Output JSON is valid and follows the schema above
- Each operation has a rationale
- Operations are compatible with the {strategy} strategy type
"#,
        strategy = slice.strategy.label(),
        data_path = data_path,
        run_id = run_id,
        skill_doc = load_skill_doc_or_default(slice.strategy),
    );

    // Create task via graph API (not wg CLI, since we're inside wg evolve)
    let task = Task {
        id: task_id.clone(),
        title: format!("Evolve analyzer: {}", slice.strategy.label()),
        description: Some(description),
        status: Status::Open,
        after: vec![partition_task_id.to_string()],
        tags: vec!["evolution".into(), "analyzer".into()],
        model: Some(model.to_string()),
        // ... other fields default
    };

    graph.add_node(Node::Task(task));
    Ok(task_id)
}
```

### 2.2 Analyzer Output Schema

Each analyzer writes a proposals JSON file. The `operations` array uses the
existing `EvolverOperation` struct (from `strategy.rs`) extended with two fields:

```rust
// Added to EvolverOperation in strategy.rs:

/// Analyzer's confidence in this operation (0.0-1.0).
/// Used by synthesizer for priority scoring.
#[serde(default, skip_serializing_if = "Option::is_none")]
pub confidence: Option<f64>,

/// Expected impact description.
#[serde(default, skip_serializing_if = "Option::is_none")]
pub expected_impact: Option<String>,
```

### 2.3 Model Tier Assignments

| Strategy | Model | Rationale |
|----------|-------|-----------|
| Mutation | Sonnet | Balanced analysis of moderate-score roles |
| Crossover | Sonnet | Pattern matching across role pairs |
| Gap Analysis | Opus | Structural reasoning about coverage gaps |
| Retirement | Haiku | Mechanical scan against clear thresholds |
| Motivation Tuning | Sonnet | Correlation analysis |
| Component Mutation | Sonnet | Component-level performance analysis |
| Randomisation | Haiku | Combinatorial selection, not deep analysis |
| Bizarre Ideation | Opus | Requires creativity and divergent thinking |

These are defaults. The evolver's own model config (`config.resolve_model_for_role`)
takes precedence if set. The `ModelTier` is a suggestion recorded in the task's
`model` field.

---

## 3. Synthesizer Merge Algorithm

### 3.1 Synthesizer Task

`.evolve-synthesize-{run_id}` depends on ALL analyzer tasks (`--after` all of them).
It reads all `*-proposals.json` files and produces a unified operation set.

### 3.2 Merge Algorithm

```rust
// In src/commands/evolve/synthesize.rs (new file)

pub struct SynthesizedPlan {
    pub operations: Vec<EvolverOperation>,
    pub conflicts_resolved: Vec<ConflictResolution>,
    pub budget_cuts: Vec<BudgetCut>,
    pub total_proposed: usize,
    pub total_accepted: usize,
}

pub struct ConflictResolution {
    pub entity_id: String,
    pub competing_ops: Vec<(Strategy, String)>, // (strategy, op_type)
    pub winner: Strategy,
    pub reason: String,
}

pub struct BudgetCut {
    pub operation: EvolverOperation,
    pub strategy: Strategy,
    pub reason: String,
}

pub fn synthesize(
    proposal_files: &[PathBuf],
    budget: Option<u32>,
    roles: &[Role],
    tradeoffs: &[TradeoffConfig],
) -> Result<SynthesizedPlan> {
    // 1. Load all proposals
    let mut all_ops: Vec<(Strategy, EvolverOperation)> = Vec::new();
    for file in proposal_files {
        let proposals: AnalyzerProposals = load_json(file)?;
        for op in proposals.operations {
            all_ops.push((proposals.strategy, op));
        }
    }

    // 2. Deduplication
    let deduped = deduplicate(&all_ops);

    // 3. Conflict resolution
    let resolved = resolve_conflicts(&deduped, roles, tradeoffs);

    // 4. Priority scoring
    let scored = score_operations(&resolved);

    // 5. Budget enforcement
    let final_ops = enforce_budget(&scored, budget);

    Ok(SynthesizedPlan { ... })
}
```

### 3.3 Deduplication Rules

Two operations target the same entity if:
- Both have the same `target_id`, OR
- One creates and another modifies/retires the same `new_id`/`target_id`

Deduplication priority (when same entity targeted by same operation type from
multiple strategies):
1. Higher `confidence` score wins.
2. On tie, the strategy with more data (more evals in its slice) wins.
3. On tie, prefer the more conservative operation.

### 3.4 Conflict Resolution Matrix

When different operation types target the same entity:

| Operation A | Operation B | Resolution |
|-------------|-------------|------------|
| `modify_role` | `retire_role` | **Retire wins** if `confidence ≥ 0.7` AND role `avg_score < 0.30`. Otherwise modify wins (give it a chance). |
| `modify_role` | `modify_role` | Keep the one with higher confidence. If within 0.1, merge: use the modification with the most changed fields. |
| `create_role` | `retire_role` (different entity) | Both proceed — net zero. |
| `modify_motivation` | `retire_motivation` | Same logic as role modify/retire. |
| `wording_mutation` | `component_substitution` (same role) | Both proceed — they modify different aspects. |
| `config_swap_outcome` | `modify_role` (same role) | `modify_role` subsumes — it can include the outcome change. |
| `meta_swap_*` | any targeting same meta-agent | Keep the one with higher confidence. |

### 3.5 Priority Scoring

Each operation receives a composite priority score:

```
priority = confidence × 0.4
         + signal_strength × 0.3
         + expected_impact × 0.2
         + novelty × 0.1
```

Where:
- `confidence`: From analyzer (0.0–1.0)
- `signal_strength`: `min(1.0, eval_count_for_target / 10.0)` — more evals = stronger signal
- `expected_impact`: Estimated score delta. For retirements: `1.0 - avg_score`.
  For mutations: `abs(avg_score - 0.5)` (further from mediocre = more room to improve).
  For gap fills: `0.7` (default — uncertain but valuable).
- `novelty`: `1.0 - (generation / 10.0).min(1.0)` — prefer evolving younger entities

### 3.6 Budget Enforcement

Default budget: `max(5, total_roles / 3)` operations per evolution run.
Configurable via `--budget N` or `config.agency.evolution_budget`.

After sorting by priority descending:
1. Take top N operations.
2. Ensure at least 1 operation from each strategy that proposed any (if budget allows).
3. Never exceed `total_roles * 0.5` retirements in a single run (stability guard).
4. Record cut operations in `budget_cuts` for transparency.

### 3.7 Synthesizer Output

Written to `.wg/evolve-runs/{run_id}/synthesis-result.json`:

```json
{
  "run_id": "run-20260313-144800",
  "operations": [ ... ],
  "conflicts_resolved": [
    {
      "entity_id": "abc123",
      "competing_ops": [["mutation", "modify_role"], ["retirement", "retire_role"]],
      "winner": "mutation",
      "reason": "Role avg_score=0.42 > retirement threshold; confidence for modify (0.8) > retire (0.6)"
    }
  ],
  "budget_cuts": [ ... ],
  "stats": {
    "total_proposed": 23,
    "total_accepted": 8,
    "strategies_represented": ["mutation", "crossover", "retirement"],
    "conflicts_count": 3
  }
}
```

The synthesizer task is an LLM task (model: Sonnet) that reads all proposal files
and the conflict resolution rules above. It can override the mechanical rules when
it has good reason, documented in its rationale. It writes the final
`synthesis-result.json`.

---

## 4. Cycle Configuration

### 4.1 Task Graph Structure

```
.evolve-partition-{run_id}
    │
    ├──► .evolve-analyze-*-{run_id}  (N parallel tasks)
    │
    └──► .evolve-synthesize-{run_id}  (--after all analyzers)
            │
            └──► .evolve-apply-{run_id}
                    │
                    └──► .evolve-evaluate-{run_id}
                            │
                            └──► [back-edge to .evolve-partition-{run_id}]
```

### 4.2 Non-Cycle Mode (Default)

By default, `wg evolve run` creates the pipeline **without** the back-edge.
This is a single pass: partition → analyze → synthesize → apply → done.

The `.evolve-apply` task:
- Reads `synthesis-result.json`
- Calls existing `apply_operation()` for each operation
- Records results in `.wg/evolve-runs/{run_id}/apply-results.json`
- Is a **Rust code task** (not LLM) — it calls the existing apply functions

The `.evolve-evaluate` task:
- Compares pre-evolution performance snapshot with post-evolution state
- Records which operations were applied vs skipped (deferred, no-op, error)
- Writes `.wg/evolve-runs/{run_id}/evolution-report.json`
- Is an **LLM task** (model: Sonnet) that reads the apply results and produces
  a human-readable assessment

### 4.3 Autopoietic Cycle Mode

Enabled via `wg evolve run --autopoietic` (or `--cycle`).

Creates the same pipeline but with a back-edge from `.evolve-evaluate` to
`.evolve-partition`, with cycle configuration:

```rust
CycleConfig {
    max_iterations: 3,       // default, configurable via --max-iterations
    cycle_delay_secs: 3600,  // 1 hour default — wait for new evals to accumulate
    cycle_guard: Some(".evolve-partition-{run_id}".to_string()),
}
```

**Convergence criteria** (checked by `.evolve-evaluate`):
1. **No operations applied**: Synthesizer found nothing worth doing → converged.
2. **Score plateau**: Average score delta < 0.02 across all modified entities → converged.
3. **Diminishing returns**: Each iteration applied fewer operations than the previous → converged.

When converged, `.evolve-evaluate` runs `wg done --converged` on itself to
stop the cycle.

### 4.4 Cycle Delay

The `cycle_delay_secs` serves a critical purpose: after applying evolution
operations, the new/modified roles need to be **assigned to tasks and evaluated**
before the next cycle has useful data. Without delay, the next iteration would
see the same pre-evolution data.

Default: 3600 seconds (1 hour). Configurable via `--cycle-delay <seconds>`.

For testing: `--cycle-delay 0` runs iterations immediately.

### 4.5 Pre/Post Snapshot

Before the partition task runs, it saves a performance snapshot:

```json
{
  "timestamp": "2026-03-13T14:48:00Z",
  "iteration": 0,
  "roles": {
    "abc123": { "avg_score": 0.65, "task_count": 12 },
    ...
  },
  "tradeoffs": {
    "def456": { "avg_score": 0.70, "task_count": 8 },
    ...
  },
  "overall_avg": 0.672
}
```

Stored in `.wg/evolve-runs/{run_id}/snapshot-iter-{N}.json`.
The evaluate task compares consecutive snapshots.

---

## 5. Self-Modification Safety

### 5.1 Skill Doc Evolution

Analyzers can propose changes to their own strategy skill documents. This is the
primary self-modification mechanism.

**How it works:**
- Each analyzer's output JSON includes an optional `skill_doc_update` field:

```json
{
  "operations": [ ... ],
  "skill_doc_update": {
    "filename": "role-mutation.md",
    "proposed_content": "# Role Mutation Strategy\n\n...",
    "rationale": "Added section on component-aware mutation based on observed patterns"
  }
}
```

- The synthesizer collects skill doc proposals and includes them in the apply step.
- Skill doc updates are **always applied** (they're documentation, not identity changes).
- Previous versions are archived: `evolver-skills/role-mutation.md.bak.{timestamp}`.

### 5.2 Meta-Agent Evolution (Existing Safety)

The existing self-mutation safety already handles this:
- Operations targeting the evolver's own role/tradeoff → deferred to a verified
  wg task requiring human approval (via `defer_self_mutation()`).
- `meta_swap_*` / `meta_compose_agent` on `meta_role: "evolver"` → deferred.

No changes needed to this mechanism. It applies equally in fan-out mode since
the `.evolve-apply` task calls the same `apply_operation()` function.

### 5.3 Cycle Parameter Self-Tuning

The `.evolve-evaluate` task can propose cycle parameter adjustments for the NEXT
evolution run (not the current one). These are recorded as recommendations in
the evolution report:

```json
{
  "cycle_recommendations": {
    "max_iterations": 4,
    "cycle_delay_secs": 7200,
    "budget": 12,
    "rationale": "Last cycle converged in 2 iterations with minimal impact; suggest longer delay for more eval accumulation"
  }
}
```

These recommendations are **advisory only**. They're stored in the report but
not automatically applied. A future enhancement could auto-apply them with
human approval.

### 5.4 Evolution History

Each run creates a persistent record in `.wg/evolve-runs/{run_id}/`:

```
.wg/evolve-runs/
  └── run-20260313-144800/
      ├── config.json           # Run parameters (strategy, budget, model, cycle config)
      ├── snapshot-iter-0.json   # Pre-evolution performance snapshot
      ├── mutation-slice.json    # Partition output
      ├── crossover-slice.json
      ├── ...
      ├── mutation-proposals.json    # Analyzer outputs
      ├── crossover-proposals.json
      ├── ...
      ├── synthesis-result.json     # Synthesizer output
      ├── apply-results.json        # Application results
      ├── snapshot-iter-1.json      # Post-evolution snapshot (if cycle)
      └── evolution-report.json     # Final evaluation report
```

This history enables:
- Trend analysis across runs
- Identifying which strategies produce the most impact
- Detecting regression (operations that made things worse)

---

## 6. Integration with Existing `wg evolve run`

### 6.1 Decision Logic in `run()`

```rust
pub fn run(
    dir: &Path,
    dry_run: bool,
    strategy: Option<&str>,
    budget: Option<u32>,
    model: Option<&str>,
    json: bool,
    autopoietic: bool,   // new flag: --autopoietic / --cycle
    max_iterations: Option<u32>,  // new flag: --max-iterations
    cycle_delay: Option<u64>,     // new flag: --cycle-delay
) -> Result<()> {
    // ... existing setup (load roles, tradeoffs, evaluations) ...

    let eval_count = evaluations.len();

    // Threshold: use legacy single-shot mode for small eval sets
    const FANOUT_THRESHOLD: usize = 50;

    if eval_count < FANOUT_THRESHOLD && !autopoietic {
        // === Legacy single-shot mode ===
        // Existing code path: build prompt, call claude, parse, apply
        return run_single_shot(dir, dry_run, strategy, budget, model, json,
                               &roles, &tradeoffs, &evaluations, &config);
    }

    // === Fan-out mode ===
    return run_fanout(dir, dry_run, strategy, budget, model, json,
                      autopoietic, max_iterations, cycle_delay,
                      &roles, &tradeoffs, &evaluations, &config);
}
```

### 6.2 Fan-Out Mode Implementation

```rust
fn run_fanout(
    dir: &Path,
    dry_run: bool,
    strategy: Option<&str>,
    budget: Option<u32>,
    model: Option<&str>,
    json: bool,
    autopoietic: bool,
    max_iterations: Option<u32>,
    cycle_delay: Option<u64>,
    roles: &[Role],
    tradeoffs: &[TradeoffConfig],
    evaluations: &[Evaluation],
    config: &Config,
) -> Result<()> {
    let run_id = format!("run-{}", chrono::Utc::now().format("%Y%m%d-%H%M%S"));

    // 1. Create run directory
    let run_dir = dir.join(format!("evolve-runs/{}", run_id));
    fs::create_dir_all(&run_dir)?;

    // 2. Save run config
    save_run_config(&run_dir, strategy, budget, model, autopoietic, max_iterations, cycle_delay)?;

    // 3. Save pre-evolution snapshot
    save_snapshot(&run_dir, 0, roles, tradeoffs)?;

    // 4. Partition evaluations
    let strategies_to_run = match strategy {
        Some(s) => vec![Strategy::from_str(s)?],
        None => Strategy::all_individual(), // all 8 strategies
    };

    let slices = partition_evaluations(
        evaluations, roles, tradeoffs,
        &dir.join("agency"), config,
    );

    // Filter to requested strategies
    let slices: Vec<_> = slices.into_iter()
        .filter(|s| strategies_to_run.contains(&s.strategy) || strategy.is_none())
        .filter(|s| !s.evaluations.is_empty() || s.strategy.needs_no_evals())
        .collect();

    if slices.is_empty() {
        println!("No strategies have actionable data. Nothing to evolve.");
        return Ok(());
    }

    // 5. Write slice data files
    for slice in &slices {
        let path = run_dir.join(format!("{}-slice.json", slice.strategy.label()));
        fs::write(&path, serde_json::to_string_pretty(&slice)?)?;
    }

    if dry_run {
        print_fanout_dry_run(&slices, &run_id, budget, autopoietic, json);
        return Ok(());
    }

    // 6. Create partition task (already done implicitly — it's `wg evolve run` itself)
    let partition_task_id = format!(".evolve-partition-{}", run_id);
    create_partition_task(&partition_task_id, &run_id, dir)?;

    // 7. Create analyzer tasks
    let mut analyzer_task_ids = Vec::new();
    for slice in &slices {
        let task_id = create_analyzer_task(slice, &run_id, &partition_task_id, dir, config)?;
        analyzer_task_ids.push(task_id);
    }

    // 8. Create synthesize task (depends on all analyzers)
    let synthesize_task_id = format!(".evolve-synthesize-{}", run_id);
    create_synthesize_task(&synthesize_task_id, &run_id, &analyzer_task_ids, budget, dir)?;

    // 9. Create apply task (depends on synthesize)
    let apply_task_id = format!(".evolve-apply-{}", run_id);
    create_apply_task(&apply_task_id, &run_id, &synthesize_task_id, dir)?;

    // 10. Create evaluate task (depends on apply)
    let evaluate_task_id = format!(".evolve-evaluate-{}", run_id);
    create_evaluate_task(&evaluate_task_id, &run_id, &apply_task_id, dir)?;

    // 11. Wire cycle back-edge if autopoietic
    if autopoietic {
        let max_iter = max_iterations.unwrap_or(3);
        let delay = cycle_delay.unwrap_or(3600);
        wire_cycle_edge(&evaluate_task_id, &partition_task_id, max_iter, delay, dir)?;
    }

    // 12. Mark partition task as done (data is partitioned)
    // The analyzer tasks are now ready for the coordinator to dispatch.

    println!("Evolution task graph created (run: {}):", run_id);
    println!("  Analyzers: {} tasks", analyzer_task_ids.len());
    for tid in &analyzer_task_ids {
        println!("    - {}", tid);
    }
    println!("  Synthesizer: {}", synthesize_task_id);
    println!("  Apply: {}", apply_task_id);
    println!("  Evaluate: {}", evaluate_task_id);
    if autopoietic {
        println!("  Cycle: {} iterations, {} second delay",
                 max_iterations.unwrap_or(3), cycle_delay.unwrap_or(3600));
    }

    Ok(())
}
```

### 6.3 `--dry-run` in Fan-Out Mode

When `--dry-run` is specified in fan-out mode:

```
=== Dry Run: wg evolve (fan-out mode) ===

Run ID:          run-20260313-144800
Strategies:      8
Total evals:     1976
Autopoietic:     no

Strategy Slices:
  mutation:           142 evals, 8 roles    (model: sonnet)
  crossover:           96 evals, 12 roles   (model: sonnet)
  gap-analysis:         0 evals (summary)   (model: opus)
  retirement:          34 evals, 3 roles    (model: haiku)
  motivation-tuning:  180 evals, 15 tradeoffs (model: sonnet)
  component-mutation:  89 evals, 22 components (model: sonnet)
  randomisation:        0 evals (inventory)  (model: haiku)
  bizarre-ideation:     0 evals (context)    (model: opus)

Task graph:
  .evolve-partition-run-20260313-144800
    ├── .evolve-analyze-mutation-run-20260313-144800
    ├── .evolve-analyze-crossover-run-20260313-144800
    ├── .evolve-analyze-gap-analysis-run-20260313-144800
    ├── .evolve-analyze-retirement-run-20260313-144800
    ├── .evolve-analyze-motivation-tuning-run-20260313-144800
    ├── .evolve-analyze-component-mutation-run-20260313-144800
    ├── .evolve-analyze-randomisation-run-20260313-144800
    └── .evolve-analyze-bizarre-ideation-run-20260313-144800
         └── .evolve-synthesize-run-20260313-144800
              └── .evolve-apply-run-20260313-144800
                   └── .evolve-evaluate-run-20260313-144800
```

### 6.4 Backwards Compatibility

| Scenario | Behavior |
|----------|----------|
| `wg evolve run` with <50 evals | Legacy single-shot mode (unchanged) |
| `wg evolve run` with ≥50 evals | Fan-out task graph mode |
| `wg evolve run --strategy mutation` | Fan-out with only the mutation analyzer |
| `wg evolve run --strategy mutation` with <50 evals | Legacy single-shot, mutation only |
| `wg evolve run --dry-run` | Shows task graph (fan-out) or prompt (legacy) |
| `wg evolve run --budget 5` | Passed to synthesizer as budget cap |
| `wg evolve run --autopoietic` | Forces fan-out mode even with <50 evals |
| `wg evolve deferred list/approve/reject` | Unchanged — deferred operations work the same |

### 6.5 New CLI Flags

```
wg evolve run [OPTIONS]

Existing:
  --strategy <STRATEGY>   Evolution strategy (default: all)
  --budget <N>            Max operations to apply
  --model <MODEL>         Override model selection
  --dry-run               Show plan without executing
  --json                  JSON output

New:
  --autopoietic           Enable autopoietic cycle mode (back-edge from evaluate to partition)
  --cycle                 Alias for --autopoietic
  --max-iterations <N>    Max cycle iterations (default: 3, requires --autopoietic)
  --cycle-delay <SECS>    Seconds between iterations (default: 3600, requires --autopoietic)
  --force-fanout          Force fan-out mode even with <50 evaluations
  --single-shot           Force legacy single-shot mode even with ≥50 evaluations
```

### 6.6 New Files

| Path | Purpose |
|------|---------|
| `src/commands/evolve/partition.rs` | Evaluation data partitioning logic |
| `src/commands/evolve/synthesize.rs` | Merge algorithm and conflict resolution |
| `src/commands/evolve/fanout.rs` | Fan-out task graph creation and wiring |
| `src/commands/evolve/snapshot.rs` | Performance snapshot save/compare |

### 6.7 Modified Files

| Path | Changes |
|------|---------|
| `src/commands/evolve/mod.rs` | Add decision logic, new flags, extract `run_single_shot()` |
| `src/commands/evolve/strategy.rs` | Add `confidence` and `expected_impact` to `EvolverOperation`, add `Strategy::all_individual()` and `Strategy::needs_no_evals()` |
| `src/commands/evolve/prompt.rs` | Refactor `build_evolver_prompt()` to work per-strategy (for analyzer task descriptions) |

---

## Appendix A: Strategy Skill Doc Templates

The 7 missing skill docs should be created. Each follows this template:

```markdown
# {Strategy Name} Strategy

## What to look for
<Criteria for identifying candidates>

## Selection criteria
<Quantitative thresholds>

## Operations available
<List of valid operation types for this strategy>

## Output expectations
<What a good set of proposals looks like>

## Anti-patterns
<Common mistakes to avoid>
```

This is out of scope for this design but is a prerequisite for the analyzers
to produce high-quality output. A separate task should create these docs.

## Appendix B: Apply Task Implementation

The `.evolve-apply` task is unique — it's a **code execution task**, not an LLM task.
It should use `exec_mode: shell` with a verification command:

```bash
wg evolve apply-synthesis --run-id {run_id}
```

This requires a new subcommand `wg evolve apply-synthesis` that:
1. Reads `synthesis-result.json`
2. Calls `apply_operation()` for each operation
3. Handles deferred operations (self-mutation safety)
4. Writes `apply-results.json`

This keeps the apply step deterministic and fast (no LLM needed).

## Appendix C: Context Budget Calculations

Based on Claude model context windows (~200K tokens):

| Component | Tokens (approx) |
|-----------|-----------------|
| System prompt + identity | ~2K |
| Strategy instructions | ~2K |
| Skill document | ~1K |
| Output format spec | ~1K |
| Available headroom | ~194K |
| Safety margin (50%) | ~97K |
| **Eval data budget** | **~97K tokens ≈ 388K chars** |
| Chars per eval (avg) | ~250 |
| **Max evals per analyzer** | **~1,550** |
| **Conservative limit** | **400** |

The 400-eval limit per analyzer provides a 4x safety margin and accounts for
role/tradeoff metadata that accompanies the evaluations.
