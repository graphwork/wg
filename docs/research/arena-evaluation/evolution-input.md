# Arena Evaluation as Evolution Input

How FLIP-style arena evaluation (Wang et al., 2025, arXiv:2602.13551) can feed into wg's agent evolution system.

## 1. Current Evolution: How `wg evolve` Works

The evolution system (`src/commands/evolve.rs`) operates as an LLM-driven optimization loop over the agency's roles and motivations:

1. **Load state**: All roles, motivations, and evaluations from `.wg/agency/`
2. **Build performance summary**: `build_performance_summary()` aggregates per-role and per-motivation scores, dimension breakdowns, and a synergy matrix (role × motivation pairings)
3. **Invoke evolver LLM**: A Claude instance receives the performance summary plus strategy-specific skill documents and returns structured JSON operations
4. **Apply operations**: `create_role`, `modify_role`, `retire_role`, `create_motivation`, `modify_motivation`, `retire_motivation`
5. **Track lineage**: Each evolved entity records `parent_ids`, `generation`, and `created_by` in its `Lineage` struct (`src/agency.rs:52`)

**Strategies** available: mutation (tweak one role), crossover (merge two motivations), gap-analysis (fill missing skill coverage), retirement (remove underperformers), motivation-tuning (adjust tradeoff boundaries).

**Key limitation**: Evolution quality is bounded by evaluation density. The evolver sees `PerformanceRecord.avg_score` and `EvaluationRef` lists, but evaluations are currently sparse — each requires an expensive LLM-as-Judge call (`wg evaluate`). With few data points, the evolver makes noisy decisions. The synergy matrix is often incomplete.

## 2. Arena as Evolution Signal

Instead of aggregate scores from individual evaluations, arena rankings provide a **relative** signal: which agent variant performs better on the same task.

**Why relative beats absolute for evolution:**
- Aggregate scores are noisy across task types (a 0.8 on a hard task ≠ 0.8 on an easy one)
- Arena rankings normalize for task difficulty automatically — both variants face the same task
- FLIP scoring is cheap (no large-model API call needed; a 1–12B model suffices) and robust against reward hacking (§5, Figure 6 of the paper)
- The paper shows FLIP outperforms LLM-as-Judge by +99.4% on average across 13 small models (Table 1, §4.1)

**Concrete change to `build_performance_summary()`**: Instead of only reporting `avg_score` per role, also report arena win-rates where available. The evolver prompt already receives role/motivation performance data — adding win-rate fields gives it a strictly better signal for deciding mutations and retirements.

## 3. Arena for A/B Testing Agent Mutations

The core use case: after `wg evolve` produces a mutated role, **arena-compare the parent and child on representative tasks** before committing to the mutation.

### Workflow

```
1. wg evolve --dry-run          → proposes: modify_role(analyst → analyst-v2)
2. wg arena analyst analyst-v2   → runs both on N sampled tasks
   - For each task:
     a. Generate response with analyst agent
     b. Generate response with analyst-v2 agent
     c. FLIP-score both: r = F1(task_description, FLIP(response))
     d. Record winner
3. If analyst-v2 win-rate > threshold → apply mutation
   If analyst-v2 win-rate ≈ analyst  → keep both (diversity)
   If analyst-v2 win-rate < threshold → discard mutation
```

### Integration with `evolve.rs`

Currently, `apply_operation()` (line 372) applies mutations immediately. An arena-gated evolution would:

1. Apply the mutation to create the child entity (with lineage tracking as today)
2. Instead of marking it active immediately, mark it `provisional`
3. Schedule arena comparison tasks via `wg add`
4. Only promote to active after arena validation passes

This mirrors the paper's Best-of-N selection (§4.2): generate N candidates, FLIP-score each, select the winner. Here N=2 (parent vs child).

## 4. Integration with Lineage Tracking

The `Lineage` struct already records `parent_ids`, `generation`, and `created_by`. Arena results should become part of this evolutionary record.

### Proposed additions to lineage

```rust
// In Lineage or as a sibling struct
pub struct ArenaResult {
    pub opponent_id: String,       // ID of the other variant
    pub task_ids: Vec<String>,     // tasks used in the arena
    pub wins: u32,
    pub losses: u32,
    pub draws: u32,
    pub flip_scores: Vec<f64>,     // per-task FLIP scores
    pub opponent_scores: Vec<f64>, // opponent's per-task FLIP scores
    pub timestamp: DateTime<Utc>,
}
```

This would be stored in `PerformanceRecord` alongside `evaluations`:

```rust
pub struct PerformanceRecord {
    pub task_count: u32,
    pub avg_score: Option<f64>,
    pub evaluations: Vec<EvaluationRef>,
    pub arena_results: Vec<ArenaResult>,  // NEW
}
```

**Benefits for evolution**:
- The evolver can see not just "analyst-v2 has avg_score 0.82" but "analyst-v2 beat analyst 7/10 on task types X, Y, Z"
- Lineage tree queries (`role_ancestry()` in `agency.rs:1099`) can show arena results at each generation, making the evolutionary trajectory auditable
- `record_evaluation()` already propagates scores to agent, role, and motivation — arena results would follow the same pattern

## 5. Requisite Variety via Arena

A risk with any optimization loop: the population converges to a single strategy. If evolution only selects for highest score, you lose specialist roles.

**Arena maintains diversity by design:**

1. **Head-to-head on task subtypes**: Run arenas within task categories (e.g., "code review" tasks, "research" tasks). A role that loses the global arena might win on its niche. The synergy matrix in `build_performance_summary()` already groups by role × motivation — arena results per task-type extend this naturally.

2. **Elo-style rating over arenas**: Instead of raw win-rates, compute an Elo rating per agent variant. This handles transitive relationships (A beats B, B beats C, but C beats A) and naturally prevents a single winner from dominating.

3. **Retirement protection**: Currently, `roles_below_threshold()` (agency.rs:1525) retires roles below a score threshold. With arena data, add a condition: don't retire a role that **wins its niche arena** even if its global avg_score is low. A specialist with 0.6 global avg but 0.9 on its task type should survive.

4. **Diversity pressure in the evolver prompt**: When building the evolver prompt, include a diversity metric (e.g., number of distinct skill sets covered, niche win-rates). Instruct the evolver: "maintain coverage across all task types; do not retire the last role covering a skill."

The paper's adversarial robustness (§5) is relevant here — FLIP scores are harder to game than LLM-judge scores, so agents can't evolve to exploit the evaluator at the expense of genuine capability.

## 6. Implementation Sketch

### Phase 1: FLIP scoring as evaluation source

- Add `source: "flip"` evaluations alongside existing `source: "llm"` evaluations
- Auto-run FLIP on every completed task (cheap; no API call to large model)
- Store with `source: "flip-auto"` in the `Evaluation` struct
- Evolution immediately benefits from denser performance data

### Phase 2: Arena comparison command

```
wg arena <variant-a> <variant-b> [--tasks N] [--task-type TYPE]
```

- Sample N completed tasks matching the type filter
- Re-run both variants (or use cached outputs if available)
- FLIP-score each pair, record results
- Output: win-rate, per-task scores, recommendation (promote/keep-both/discard)
- Store `ArenaResult` in both variants' `PerformanceRecord`

### Phase 3: Arena-gated evolution

- `wg evolve` gains a `--arena-validate` flag
- Mutations are applied provisionally
- Arena comparison runs automatically against parent
- Only promoted mutations are marked active; failed ones are retired with `rationale: "lost arena vs parent"`
- Lineage records the arena result at each evolutionary step

### Phase 4: Elo ratings and diversity tracking

- Compute Elo ratings from accumulated arena results
- Add diversity metrics to `build_performance_summary()`
- Evolver prompt includes Elo ratings and niche specialization data
- Retirement decisions factor in niche arena performance, not just global averages

### Key files to modify

| File | Change |
|------|--------|
| `src/agency.rs` | Add `ArenaResult` struct, extend `PerformanceRecord`, add arena recording functions |
| `src/commands/evaluate.rs` | Add `--method flip` path for FLIP-based evaluation |
| `src/commands/evolve.rs` | Add `--arena-validate` flag, include arena win-rates in `build_performance_summary()` |
| `src/commands/arena.rs` (new) | `wg arena` command implementation |
| `src/commands/service.rs` | Hook auto-FLIP evaluation into task completion |
