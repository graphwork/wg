# Agent Self-Validation Before `wg done`

**Author**: scout (analyst)
**Date**: 2026-02-25
**Task**: research-agent-self

---

## 1. Current State: What Validation Guidance Do Agents Receive?

### 1.1 Prompt Template (executor.rs)

The `REQUIRED_WORKFLOW_SECTION` constant in `src/service/executor.rs:20-50` is the only guidance agents receive about task completion. It says:

```
## Important
- Run `wg log` commands BEFORE doing work to track progress
- Run `wg done` BEFORE you finish responding
- If the task description is unclear, do your best interpretation
```

**There is zero guidance on validation before calling `wg done`.** The prompt tells agents *how* to mark done but not *what to verify first*.

### 1.2 AGENT-GUIDE.md

`docs/AGENT-GUIDE.md` is a comprehensive guide on graph patterns, agency, and control. It covers:
- Pattern recognition (pipeline, diamond, loop, etc.)
- Stigmergic coordination
- Convergence signaling
- Anti-patterns

**Nowhere in the guide does it mention validation steps agents should perform before marking work complete.** The "Manual Operation" section (§8) shows the workflow as:

```bash
# ... do the work ...
wg log <task-id> "What I did"
wg artifact <task-id> path/to
wg done <task-id>
```

No validation between "do the work" and "wg done."

### 1.3 Wrapper Script (spawn.rs:743-783)

The `run.sh` wrapper script auto-completes tasks if the agent exits successfully without calling `wg done` itself:

```bash
if [ "$TASK_STATUS" = "in-progress" ]; then
    if [ $EXIT_CODE -eq 0 ]; then
        wg done "$TASK_ID"
    else
        wg fail "$TASK_ID" --reason "Agent exited with code $EXIT_CODE"
    fi
fi
```

**The wrapper performs no validation.** A clean exit (code 0) = done, regardless of work quality.

### 1.4 `verify` Field (graph.rs:208)

The Task struct has a `verify: Option<String>` field documented as "Verification criteria - if set, task requires review before done." However, this field is **effectively unused** — the `done.rs` command ignores it completely (the submit command was deprecated, and `wg done` works regardless of whether `verify` is set). The only trace is in `spawn.rs` tests showing it once had a role in wrapper script behavior.

### 1.5 Post-Completion Evaluation

The agency system runs evaluations *after* `wg done` via `capture_task_output()` (done.rs:196-209). This captures git diffs, artifacts, and logs, then the coordinator spawns an evaluator task. But this is **retroactive** — it grades work after the fact, too late to prevent a bad completion.

### 1.6 Summary

| Component | Validation Guidance | Enforcement |
|-----------|-------------------|-------------|
| Prompt template | None | None |
| AGENT-GUIDE.md | None | None |
| Wrapper script | Exit code only | Mechanical only |
| `verify` field | Exists but unused | None |
| Evaluation system | Post-completion scoring | Retroactive only |

**The gap is clear: agents receive no guidance on what to validate, and the system has no mechanism to enforce validation before completion.**

---

## 2. What Agents SHOULD Validate (By Task Type)

### 2.1 Code Change Tasks

Tasks that modify source code (identified by: artifacts include source files, task description mentions "implement", "fix", "refactor", "add feature").

**Validation checklist:**

1. **Compilation check** — Run `cargo build` (or equivalent for the project's language). Code that doesn't compile is never acceptable.
   ```bash
   cargo build 2>&1 | tail -20
   ```

2. **Run relevant tests** — At minimum, run tests related to the changed code. For Rust projects:
   ```bash
   cargo test --lib 2>&1 | tail -30
   # Or for specific modules:
   cargo test <module_name> 2>&1 | tail -30
   ```

3. **No regression check** — If the task modified existing behavior, run the full test suite:
   ```bash
   cargo test 2>&1 | tail -50
   ```

4. **Verify artifacts are registered** — Every file created or substantially modified should be recorded:
   ```bash
   wg artifact <task-id> path/to/changed/file
   ```

5. **Sanity-read changed files** — Re-read the key files to confirm they look correct and complete. Agents should not trust their memory of what they wrote.

**Self-validation log message:**
```bash
wg log <task-id> "Validation: cargo build succeeded, cargo test passed (N tests), artifacts registered"
```

### 2.2 Research/Analysis Tasks

Tasks that produce documentation or analysis (identified by: task title includes "research", "analyze", "investigate"; artifacts are .md files).

**Validation checklist:**

1. **Completeness check** — Re-read the task description and verify every requested deliverable is addressed. This is the most common failure mode for research tasks — agents answer part of the question and call done.

2. **File exists and is non-trivial** — The output document should actually exist and have substantive content:
   ```bash
   wc -l docs/research/output.md  # Should be more than a few lines
   ```

3. **Source verification** — If the research references specific files, code, or prior work, verify those references exist:
   ```bash
   # If you cite "see src/graph.rs line 205", verify that's actually there
   head -210 src/graph.rs | tail -10
   ```

4. **Internal consistency** — Re-read the document and check that claims in the summary match the analysis body. Agents often write summaries that contradict their own findings.

5. **Links and references** — Verify all file paths mentioned in the document actually exist:
   ```bash
   # For each referenced path in the document
   ls -la path/referenced/in/doc
   ```

**Self-validation log message:**
```bash
wg log <task-id> "Validation: deliverable covers all N requested items, M sources verified, document is L lines"
```

### 2.3 Documentation Tasks

Tasks that update documentation (identified by: artifacts include .md files in docs/, task mentions "document", "write guide").

**Validation checklist:**

1. **Accuracy against code** — If the documentation describes code behavior, verify the code actually does what the docs say:
   ```bash
   # Example: if docs say "wg done --converged stops the loop"
   grep -n "converged" src/commands/done.rs | head -5
   ```

2. **Command examples work** — If the docs include `wg` command examples, verify they are syntactically valid:
   ```bash
   wg --help | grep <subcommand>
   ```

3. **No broken internal links** — Check that referenced documents exist:
   ```bash
   # For each [link](path) in the markdown
   ls docs/referenced-doc.md
   ```

4. **Formatting check** — Skim the rendered structure: headers are properly nested, code blocks are fenced, tables are aligned.

### 2.4 Integration/Synthesis Tasks

Tasks that combine work from multiple upstream tasks (identified by: multiple `--after` dependencies, title includes "integrate", "synthesize").

**Validation checklist:**

1. **All upstream artifacts consumed** — Check `wg context <task-id>` and verify every upstream artifact was read and incorporated.

2. **No conflicts** — If multiple upstream tasks modified adjacent areas, verify the integration doesn't introduce inconsistencies.

3. **The whole compiles/works** — After integration, the combined result should be functional:
   ```bash
   cargo build && cargo test
   ```

---

## 3. Enforcement Mechanisms

### 3.1 Prompt-Level Guidance (Recommended — Low Effort)

Add a "Pre-Completion Validation" section to the prompt template between the current "Required Workflow" and "Important" sections. This is the highest-impact change for lowest effort.

**Proposed addition to `REQUIRED_WORKFLOW_SECTION` in executor.rs:**

```
## Before Completing Your Task

Validate your work BEFORE calling `wg done`:

**For code changes:**
1. Run the build: `cargo build` (or equivalent)
2. Run tests: `cargo test` (at minimum, tests related to your changes)
3. Re-read your key changes to verify correctness
4. Register all modified files as artifacts

**For research/documentation:**
1. Re-read the task description — did you address every point?
2. Verify file paths and code references you cited actually exist
3. Check that your summary matches your analysis

**Log what you validated:**
```bash
wg log {{task_id}} "Validation: <what you checked and the result>"
```
```

### 3.2 Validation Log Requirement (Recommended — Medium Effort)

Make `wg done` check that the task has at least one log entry containing "Validation:" (or similar marker) before allowing completion.

**Implementation sketch in done.rs:**

```rust
// Check for validation log entry
let has_validation_log = task.log.iter().any(|entry| {
    entry.message.to_lowercase().contains("validation:")
});

if !has_validation_log {
    eprintln!(
        "Warning: No validation log entry found for '{}'. \
         Consider running validation checks and logging results with: \
         wg log {} \"Validation: ...\"",
        id, id
    );
    // Warning only — don't block completion
}
```

This should be a **warning, not a hard block**, because:
- Some tasks genuinely don't need validation (trivial edits, metadata changes)
- Hard blocks frustrate agents and lead to fake validation messages
- The warning creates a social norm without breaking workflows

### 3.3 Task-Type-Aware Validation (Future — Higher Effort)

Use the task's tags and skills to determine which validation steps apply, then check them. For example:

- Task has tag `code` or skill `rust` → check that `cargo build` appears in logs
- Task has tag `research` → check that artifact files exist and are non-empty
- Task has tag `docs` → check that artifact .md files reference existing code paths

This could be a `wg validate <task-id>` command that runs type-appropriate checks and produces a report, which agents call before `wg done`.

### 3.4 Wrapper Script Validation Hook (Future — Medium Effort)

Add a validation step to `run.sh` before auto-completion. After the agent exits, before calling `wg done`:

```bash
# Run validation if task has code artifacts
if wg show "$TASK_ID" --json 2>/dev/null | grep -q '"artifacts"'; then
    echo "[wrapper] Running post-agent validation..." >> "$OUTPUT_FILE"
    # Check compilation
    if [ -f "Cargo.toml" ]; then
        cargo build 2>> "$OUTPUT_FILE"
        if [ $? -ne 0 ]; then
            echo "[wrapper] Build failed, marking task failed" >> "$OUTPUT_FILE"
            wg fail "$TASK_ID" --reason "Post-agent build check failed"
            exit 1
        fi
    fi
fi
```

**Caution:** This only catches cases where the agent didn't call `wg done` itself (the wrapper auto-completes). Most well-behaved agents call `wg done` directly, bypassing the wrapper's post-exit logic.

### 3.5 Revive the `verify` Field (Future — Medium Effort)

The existing `task.verify` field could be repurposed as a validation specification:

```bash
wg add "Implement auth" --verify "cargo test auth && cargo clippy"
```

When `wg done` is called on a task with `verify` set, run the verification command and fail if it returns non-zero. This gives task authors explicit control over validation criteria.

---

## 4. Prior Art

### 4.1 CI/CD Systems

CI/CD pipelines are the closest analog — they enforce validation gates before deployment:

| System | Validation Pattern | Lesson for workgraph |
|--------|-------------------|---------------------|
| **GitHub Actions** | Jobs define `steps` that must all pass before the workflow succeeds. Matrix strategies run parallel checks (build, test, lint). | Tasks could define a `checks` list that must pass before `wg done` is accepted. |
| **GitLab CI** | `rules` and `needs` create DAG-based pipelines with explicit stage gates. Stages only proceed if all prior stage jobs pass. | The `after` dependency mechanism already provides this; what's missing is intra-task validation gates. |
| **Jenkins** | `post { always { ... } success { ... } failure { ... } }` blocks run cleanup/validation after stages. | The wrapper script could implement `post`-style hooks. |
| **Argo Workflows** | Each step can have `exit-handler` and `retry` strategies. Steps can define `outputs` that downstream steps validate. | Combine with artifact registration — downstream tasks validate upstream artifacts. |

**Key lesson:** CI/CD systems separate "did it run" from "did it succeed." workgraph currently conflates these — a clean agent exit = success. CI adds an explicit check phase.

### 4.2 Multi-Agent Frameworks

| Framework | Validation Approach | Lesson |
|-----------|-------------------|--------|
| **AutoGen (Microsoft)** | "Critic" agents review work before acceptance. Nested chat with validation loop. | workgraph's evaluation system is similar but post-hoc. Could add pre-completion evaluation. |
| **CrewAI** | Tasks have `expected_output` field. Output is validated against expectations before proceeding. | The `verify` field could serve this purpose if revived. |
| **LangGraph** | Nodes can have "conditional edges" — work flows to different nodes based on validation results. | Loop guards already implement this pattern for cycles. Extend to linear tasks? |
| **MetaGPT** | Role-based agents with mandatory review stages (e.g., QA Engineer reviews Developer output). | Scatter-gather pattern already supports this. Task authors should add review tasks. |
| **Devin / SWE-agent** | Runs tests as part of its edit-test loop. Doesn't submit until tests pass. | Closest to what we need — agents should build+test before marking done. |

**Key lesson:** The most effective multi-agent systems make validation **intrinsic to the agent's task loop**, not an external gate. Agents that build-test-iterate produce better output than agents that are post-hoc graded.

### 4.3 Human Software Engineering

Software engineers validate their work before submitting PRs through an internalized checklist:

1. Does it compile?
2. Do the tests pass?
3. Did I address all requirements?
4. Did I create any regressions?
5. Is the code clean enough?

This checklist is **cultural** — enforced by team norms, not tools. PRs that skip these steps get rejected in review, training engineers to self-validate. The evaluation system serves the same function for agents (low scores = negative feedback), but the feedback loop is slow.

**Key lesson:** The fastest path to agent self-validation is **prompt guidance** that instills the checklist habit, just as team norms instill it in humans. Tooling enforcement is a safety net, not the primary mechanism.

---

## 5. Concrete Recommendations for AGENT-GUIDE.md

### 5.1 Add a "Self-Validation" Section (§4.5 or new §10)

```markdown
## Self-Validation: Check Before You Ship

Before calling `wg done`, validate your work. The specific checks depend on your task type.

### Code Changes

Run these commands and verify they succeed:

```bash
# 1. Build check — code must compile
cargo build 2>&1 | tail -5
# (or the project's equivalent: npm run build, go build, etc.)

# 2. Test check — related tests must pass
cargo test <relevant_module> 2>&1 | tail -20

# 3. Full test suite — no regressions
cargo test 2>&1 | tail -10

# 4. Register artifacts
wg artifact <task-id> path/to/changed/file.rs

# 5. Log your validation
wg log <task-id> "Validation: build OK, N tests passed, artifacts registered"
```

If the build or tests fail, fix the issues before marking done. If you cannot fix them,
use `wg fail` with a clear reason.

### Research & Analysis

```bash
# 1. Completeness — re-read the task description
wg show <task-id>
# Did you address every bullet point?

# 2. Verify references — do cited files/paths exist?
ls path/to/referenced/file

# 3. Check output size — is it substantive?
wc -l docs/research/your-output.md

# 4. Register artifacts
wg artifact <task-id> docs/research/your-output.md

# 5. Log your validation
wg log <task-id> "Validation: all N deliverables addressed, M references verified"
```

### Documentation

```bash
# 1. Accuracy — verify code references match reality
grep -n "referenced_function" src/relevant_file.rs

# 2. Commands — verify examples are valid
wg <subcommand> --help

# 3. Register artifacts
wg artifact <task-id> docs/your-doc.md

# 4. Log your validation
wg log <task-id> "Validation: code references verified, command syntax checked"
```

### The Validation Log

Always log what you validated. This helps:
- **Evaluators** score your thoroughness
- **Downstream agents** trust your output
- **Debugging** when something goes wrong later

Format: `wg log <task-id> "Validation: <concise summary of what was checked>"`
```

### 5.2 Add Validation to the Prompt Template

Add to `REQUIRED_WORKFLOW_SECTION` in `src/service/executor.rs`, between steps 2 and 3:

```
3. **Validate your work** before completing:
   - Code tasks: verify compilation (`cargo build`) and tests (`cargo test`)
   - Research tasks: re-read the task description and verify all points are addressed
   - All tasks: log what you validated with `wg log {{task_id}} "Validation: ..."`
```

Renumber existing steps 3 and 4 to 4 and 5.

### 5.3 Implement Soft Warning in `wg done`

Add a warning (not a hard block) when `wg done` is called without a validation log entry. This reinforces the habit without breaking workflows. See §3.2 for implementation sketch.

### 5.4 Priority Order

| Priority | Action | Effort | Impact |
|----------|--------|--------|--------|
| **P0** | Add validation section to AGENT-GUIDE.md (§5.1) | Low (docs only) | High — agents that read the guide will self-validate |
| **P0** | Add validation step to prompt template (§5.2) | Low (~10 lines) | High — every spawned agent sees validation instructions |
| **P1** | Soft warning in `wg done` (§5.3) | Medium (~20 lines) | Medium — reinforces the norm via tooling |
| **P2** | Task-type-aware `wg validate` command (§3.3) | Higher (~200 lines) | Medium — useful but agents can do this manually |
| **P3** | Revive `verify` field for executable checks (§3.5) | Medium (~100 lines) | Lower — requires task authors to specify checks |

---

## 6. Open Questions

1. **Should validation be blocking?** A hard gate in `wg done` would prevent sloppy completions but would also frustrate agents on tasks where validation isn't applicable (e.g., pure metadata tasks). Recommendation: start with soft warnings, escalate to hard blocks only for tasks with explicit `verify` criteria.

2. **What about false confidence?** Agents could write "Validation: all checks passed" without actually running checks. The evaluation system can catch this post-hoc (evaluator can verify claims), but there's no real-time enforcement. This is analogous to CI — you can't stop someone from typing "tests pass" in a PR description without actually running tests. The solution is culture (prompt guidance) + audit (evaluation).

3. **Per-project validation rules?** Different projects have different build/test commands. The prompt template currently uses `cargo` examples. A project-level config (`config.toml: [validation] build_cmd = "npm run build"`) would make validation guidance project-aware. This is a future enhancement.

4. **Should the wrapper script validate?** Adding build checks to `run.sh` catches agents that don't self-validate, but it only works for the auto-completion path (agent didn't call `wg done` itself). Most well-functioning agents call `wg done` directly, bypassing wrapper validation. The wrapper is a safety net, not the primary mechanism.
