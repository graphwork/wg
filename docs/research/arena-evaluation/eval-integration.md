# Arena Evaluation Integration with wg

Analysis of how FLIP (Wang et al., 2025, arXiv:2602.13551) integrates with wg's evaluation system.

## 1. Current Evaluation System

### How `wg evaluate` works

`wg evaluate <task-id>` (evaluate.rs:36) uses LLM-as-Judge:

1. **Gate**: Task must be `Done` or `Failed` (line 54).
2. **Context assembly**: Loads the task's agent, role, and motivation from `.wg/agency/`. Builds an `EvaluatorInput` struct (agency.rs:369) containing task title, description, skills, verify criteria, agent identity, artifacts, and log entries.
3. **Prompt rendering**: `render_evaluator_prompt()` (agency.rs:398) produces a self-contained prompt instructing the LLM to return JSON `{score, dimensions, notes}`.
4. **LLM call**: Spawns `claude --print --model <model>` with the prompt (evaluate.rs:188). Model defaults to `config.agency.evaluator_model`, falling back to the task's model.
5. **Parse + record**: Extracts JSON from output, constructs an `Evaluation` struct (agency.rs:204) with `source: "llm"`, and calls `record_evaluation()` (agency.rs:1203) which:
   - Saves eval JSON to `.wg/agency/evaluations/`
   - Updates `PerformanceRecord` on the agent, role, and motivation (appends `EvaluationRef`, recalculates `avg_score`)

There is also `wg evaluate record` (evaluate.rs:327) for externally-sourced evaluations — accepts score, source string, and optional dimensions. This is the natural entry point for FLIP scores.

### Key data structures

- **`Evaluation`** (agency.rs:204): `{id, task_id, agent_id, role_id, motivation_id, score, dimensions, notes, evaluator, timestamp, model, source}`
- **`EvaluationRef`** (agency.rs:34): `{score, task_id, timestamp, context_id}` — lightweight reference stored inline on roles/motivations/agents
- **`PerformanceRecord`** (agency.rs:44): `{task_count, avg_score, evaluations: Vec<EvaluationRef>}` — aggregated stats per entity

### Current limitations

- **Cost**: Each eval requires a full LLM API call, so evaluation is opt-in and sparse.
- **Bias**: Single evaluator model introduces systematic bias. No adversarial robustness (§5, p.8 of paper).
- **No comparison**: Scores are absolute (0–1), not relative. Two agents solving the same task can't be directly compared without running both.

## 2. FLIP as Evaluation Component

### Core idea

Instead of asking an LLM "how good is this output?" (judgment), ask a small model "what instruction produced this output?" (generation) and measure instruction recovery via F1.

### Mapping to wg concepts

| FLIP concept | wg equivalent |
|---|---|
| Instruction `x` | Task description + title (`EvaluatorInput.task_description`) |
| Response `y` | Agent output: log entries + artifact contents |
| Model `φ` | Any small LM (1B–12B) — configurable, no fine-tuning |
| Score `r = F1(x, x')` | `Evaluation.score` with `source: "flip"` |

### FLIP evaluation pipeline

```
task.description → x (original instruction)
task.log + task.artifacts → y (agent response)
small_model(y) → x' (inferred instruction)
F1(x, x') → score ∈ [0, 1]
```

The backward-inference prompt (from paper §3): *"Infer a single instruction that would most plausibly generate the given response."* The response `y` is constructed by concatenating log messages and artifact contents, same as `render_evaluator_prompt()` already does for the LLM judge.

## 3. Replace or Complement?

**Complement, not replace.** Here's why:

### What FLIP measures well
- **Instruction adherence**: Did the agent do what was asked? F1 directly measures how recoverable the task description is from the output. This maps to the `correctness` and `completeness` dimensions.
- **Cheap and scalable**: No API cost with a local model. Can run on every task automatically.
- **Adversarial robustness**: FLIP is harder to game than LLM-as-Judge (§5, Figure 6).

### What FLIP does NOT measure
- **Code quality**: FLIP can't assess whether code is idiomatic, efficient, or well-structured. The `efficiency` and `style_adherence` dimensions require semantic judgment.
- **Nuanced trade-offs**: The current evaluator prompt asks the LLM to consider the agent's role, motivation, and desired outcome. FLIP's F1 metric is role-agnostic.
- **Failure analysis**: For failed tasks, LLM-as-Judge provides qualitative `notes` explaining what went wrong. FLIP returns a number.
- **Cross-language / long-response bias**: Paper acknowledges longer responses inflate FLIP scores (§5, Figure 5) and cross-language pairs break F1 (§7).

### Recommended architecture

```
Task completes
  ├─ FLIP evaluation (automatic, cheap)    → source: "flip"
  │   Measures: instruction adherence
  │   Runs: on every completed task
  │
  └─ LLM evaluation (on-demand, expensive) → source: "llm"
      Measures: quality, style, nuance
      Runs: when explicitly requested or on high-stakes tasks
```

The `Evaluation.source` field (agency.rs:227) already supports this differentiation. Evolution (`evolve.rs`) can weight sources differently.

## 4. Concrete Data Flow

```
1. Agent completes task
   └─ wg done <task-id>

2. Coordinator post-completion hook (service.rs)
   └─ Triggers FLIP evaluation automatically

3. FLIP evaluation
   a. Extract instruction: x = task.title + task.description
   b. Extract response: y = concat(task.log[].message, read(task.artifacts[]))
   c. Run backward inference: x' = small_model("Infer the instruction: " + y)
   d. Compute score: r = F1(tokenize(x), tokenize(x'))
   e. Build Evaluation {
        score: r,
        source: "flip",
        evaluator: "flip:<model-name>",
        dimensions: {
          "instruction_adherence": r,
          "precision": precision(x, x'),
          "recall": recall(x, x')
        },
        notes: format!("Inferred: {}", x')
      }

4. Record evaluation
   └─ record_evaluation() (agency.rs:1203)
       ├─ Save to .wg/agency/evaluations/
       ├─ Update agent PerformanceRecord
       ├─ Update role PerformanceRecord
       └─ Update motivation PerformanceRecord

5. Downstream consumers
   ├─ wg evaluate show --source "flip*"  → view FLIP evaluations
   ├─ wg evolve → build_performance_summary() ingests FLIP scores
   └─ wg stats → agent/role rankings include FLIP data
```

## 5. Implementation Sketch

### 5a. Changes to `evaluate.rs`

**Add `--method flip` flag** to `wg evaluate run`:

```rust
// New enum for evaluation method
enum EvalMethod { Llm, Flip }

// In run():
// If method == Flip:
//   1. Build instruction text from task.title + task.description
//   2. Build response text from task.log + artifact file contents
//   3. Construct backward-inference prompt:
//      "Given the following response, infer a single instruction that
//       would most plausibly produce it.\n\nResponse:\n{response}"
//   4. Call small model (claude --model <small> --print <prompt>)
//   5. Compute word-level F1 between instruction and model output
//   6. Record with source: "flip", evaluator: "flip:{model}"
```

**Add F1 computation** (pure function, no dependencies):

```rust
fn word_f1(reference: &str, candidate: &str) -> (f64, f64, f64) {
    let ref_tokens: HashSet<&str> = reference.split_whitespace().collect();
    let cand_tokens: HashSet<&str> = candidate.split_whitespace().collect();
    let overlap = ref_tokens.intersection(&cand_tokens).count() as f64;
    let precision = if cand_tokens.is_empty() { 0.0 } else { overlap / cand_tokens.len() as f64 };
    let recall = if ref_tokens.is_empty() { 0.0 } else { overlap / ref_tokens.len() as f64 };
    let f1 = if precision + recall == 0.0 { 0.0 } else { 2.0 * precision * recall / (precision + recall) };
    (f1, precision, recall)
}
```

This follows the paper's scoring formula exactly (§3, p.4).

### 5b. Changes to `agency.rs`

**No structural changes needed.** The `Evaluation` struct already has:
- `source: String` — use `"flip"` to distinguish from `"llm"`
- `dimensions: HashMap<String, f64>` — store `instruction_adherence`, `precision`, `recall`
- `evaluator: String` — use `"flip:<model-name>"`

The `record_evaluation()` function (agency.rs:1203) works unchanged — it's source-agnostic. `PerformanceRecord` aggregates all evaluations regardless of source.

**Optional future enhancement**: Add source-aware weighting in `update_performance()` (agency.rs:1188). Currently `avg_score` treats all evaluations equally. Could weight FLIP scores lower than LLM scores:

```rust
// Future: weighted average by source
// flip scores: weight 0.5
// llm scores: weight 1.0
// manual scores: weight 1.5
```

This is not needed for initial integration — the uniform average is a reasonable default since FLIP scores on small models correlate well with large-model LLM-as-Judge scores (Table 2, §4.1).

### 5c. Changes to `service.rs` (auto-evaluation hook)

Add a post-completion step in the coordinator loop that auto-runs FLIP evaluation:

```rust
// After agent completes task and status is set to Done:
if config.agency.auto_flip_eval.unwrap_or(false) {
    // Run FLIP evaluation (cheap, local model)
    evaluate::run(dir, &task_id, Some("flip-model"), false, false)?;
}
```

### 5d. Configuration

Add to `Config`:
```yaml
agency:
  flip_model: "local/small-model"    # Model for backward inference
  auto_flip_eval: true                # Auto-run FLIP on task completion
```

### 5e. CLI surface

```
wg evaluate run <task-id>                    # LLM eval (existing)
wg evaluate run <task-id> --method flip      # FLIP eval
wg evaluate show --source flip               # View FLIP evals
wg evaluate show --source "flip*"            # Glob match (already supported)
```

No new subcommands needed. The existing `wg evaluate record` can also be used to manually record FLIP scores from external tooling.

## Summary

| Aspect | Current (LLM-as-Judge) | With FLIP |
|---|---|---|
| Cost per eval | ~$0.01–0.10 (API call) | ~$0 (local model) |
| Eval coverage | Sparse (opt-in) | Dense (every task) |
| Measures | Quality, style, nuance | Instruction adherence |
| Adversarial robustness | Low (§5) | High (§5, Figure 6) |
| Code changes | — | evaluate.rs: +~80 lines, agency.rs: 0 lines |
| Data compatibility | source: "llm" | source: "flip" (same Evaluation struct) |

FLIP complements LLM-as-Judge by providing cheap, automatic, adversarially-robust instruction-adherence scores on every task, while LLM-as-Judge remains available for deeper qualitative evaluation when needed.
