# Terminal Bench: Agent Behavior Research — Conditions A, B, C

**Date:** 2026-04-04  
**Task:** research-analyze-tb  
**Data:** Final results from all three conditions (rerun-condition-a, rerun-condition-b, full-condition-c)

---

## Executive Summary

The TB experiment ran three conditions with 89 tasks × 3 trials each. Pass rates are statistically indistinguishable across conditions (A=52.8%, B-rerun=52.4%, C=51.1%). However, **the B-rerun and C conditions are confounded** — both ran `ConditionCAgent` with identical code and prompts. The agency system (roles, tradeoffs, assignment) was not active in any trial. Self-verification behavior exists but is ad-hoc, not autopoietic. Roughly 14-18% of trials in B/C never use wg tools despite having them, primarily because the model (minimax-m2.7) ignores optional tool instructions for simple tasks.

---

## 1. B vs C Differentiation: CONFOUNDED

### Finding: Rerun Condition B used ConditionCAgent, not ConditionBAgent

**Evidence (definitive):**

| Field | B-rerun value | C value |
|-------|--------------|---------|
| `config.agent.import_path` | `wg.adapter:ConditionCAgent` | `wg.adapter:ConditionCAgent` |
| `agent_info.name` | `wg-condition-c` | `wg-condition-c` |
| `agent_result.metadata.condition` | `C` | `C` |
| `run.sh` | `--agent-import-path "wg.adapter:ConditionCAgent"` | N/A (Harbor run via task agent) |

The run.sh header explicitly says: *"Uses ConditionCAgent (same wg tools as B, but with skill injection + planning phase) — this was intentional, labeled as a 'corrected' B run."*

**Impact:** Any comparison of rerun-condition-b vs full-condition-c is comparing **the same treatment** with different random seeds. The pass rate difference (52.4% vs 51.1%) is sampling noise.

### What B and C actually differ by (in the adapter code)

The adapter (`terminal-bench/wg/adapter.py`) has distinct prompt builders:

| Feature | `build_condition_b_prompt` (L568-591) | `build_condition_c_prompt` (L594-632) |
|---------|---------------------------------------|---------------------------------------|
| Tools | CONDITION_B_TOOLS (15 tools) | CONDITION_C_TOOLS = CONDITION_B_TOOLS |
| Prompt style | Brief "guidelines" with bullet points | **Skill injection** with explicit wg usage templates |
| Planning phase | None | **Mandatory** ("analyze the task in ONE response") |
| wg_log guidance | "Use wg_log to record progress" | Template: `wg_log("{root_task_id}", "Starting: <plan>")` |
| Decomposition | "Use wg_add to decompose complex work" | **Decision heuristic**: "If 3+ distinct phases or might exhaust context" |
| wg_done guidance | "Use wg_done when finished" | Template: `wg_done("{root_task_id}")` |
| Graph patterns | Listed (pipeline, diamond, loop) | Not listed (focus on wg usage, not theory) |

**Key difference:** C's prompt gives *concrete templates* with the root task ID pre-filled, while B's prompt gives *abstract instructions*. This is the experimental variable: explicit skill injection vs implicit tool availability.

### Recommendation

To validly compare B vs C, use the original `full-condition-b` data (which ran `ConditionBAgent` with `build_condition_b_prompt`). The rerun-condition-b data should be relabeled as "C-rerun" or "C-replicate" in all analyses.

---

## 2. WG Adoption Rate — Per-Trial Breakdown

### Aggregate adoption

| Condition | Trials | WG usage | Rate | Planning turn | WG state snapshot |
|-----------|--------|----------|------|---------------|-------------------|
| A (rerun) | 269 | 0 | 0.0% | 0 | 0 |
| B-rerun | 270 | 233 | 86.3% | 227 | 227 |
| C (full) | 266 | 219 | 82.3% | 227 | 228 |

### Root causes for non-adoption (B-rerun: 37 trials with 0 wg usage)

| Cause | Count | Description |
|-------|-------|-------------|
| Model ignores wg tools | 24 | Agent has tools, uses bash+file, never calls wg_*. Runs 4-50 turns without touching wg |
| Error/exception | 4 | Trial errored before meaningful wg usage (polyglot-c-py, feal-differential, polyglot-rust-c, write-compressor) |
| Very short trial (1 turn) | 2 | Agent barely started (polyglot-rust-c__KURAYx7: 1 turn, no tool calls) |
| Continuation batch artifact | 7 | Top-level directory entries for batch subdirectories |

### Tasks consistently avoiding wg (all 3 trials skip wg)

| Task | Turns (avg) | Pattern |
|------|-------------|---------|
| gcode-to-text | 18 | Agent uses bash/file directly; task is "simple enough" |
| polyglot-rust-c | 14 | Mixed errors + direct implementation |
| query-optimize | 14 | Agent goes straight to SQL optimization via bash |

### Non-adoption correlates with lower pass rate

| Group | Pass rate (valid) |
|-------|-------------------|
| With wg usage | 55.4% (112/202) |
| Without wg usage | 34.6% (9/26) |

This +20.8pp gap suggests wg usage (particularly logging and completion signaling) correlates with more methodical problem-solving, though the causal direction is unclear — it may be that harder tasks where agents fail also happen to overwhelm the model enough to skip optional tools.

### Why aren't 100% of agents using WG?

**Primary cause: prompt compliance gap.** Minimax-m2.7 treats wg tools as optional supplements. When the task is straightforward (fix a git repo, write a simple script), the model takes the shortest path using only bash and file tools. The Condition C prompt says "Always do this: wg_log before first action" but ~14% of agents ignore this instruction entirely.

**Evidence from transcript analysis:** In non-wg trials like `cancel-async-tasks__5Drxper` (Condition C), the planning turn says "Simple task — I'll implement directly" with no mention of wg_log or wg_done despite the prompt explicitly instructing these. The model's planning acknowledges the task complexity heuristic from the skill prompt but applies it to skip wg entirely.

**No adapter bug.** The adapter correctly sets up wg for all B/C trials (verified via workgraph_state presence in 85%+ of trials). The wg init succeeds, the tools are available, but the model doesn't call them.

### Recommendations for 100% wg quickstart

1. **Inject a mandatory first-turn wg_log call.** Instead of relying on the prompt, have the adapter automatically call `wg_log(root_task_id, "Task started")` before the first LLM turn. This seeds the wg state unconditionally.

2. **Remove wg tool optionality.** Rewrite the system prompt to make wg_log a non-optional step: "You MUST call wg_log before and after EVERY bash command. Failure to log will result in session termination." Aggressive, but the only way to force compliance from models that treat instructions as suggestions.

3. **Enforce at the adapter level.** After the agent loop, check if `wg_done` or `wg_fail` was called. If not, automatically log "Agent did not call wg_done — force-marking as done/failed based on verifier result." This gives 100% wg state capture without relying on model compliance.

4. **Consider a wg-gated termination.** The adapter currently breaks on `wg_done(root_task_id)` or on the LLM returning no tool calls. Add a check: if the agent stops without calling wg_done, inject a message "You must call wg_done or wg_fail to complete. What is the status of your work?" This gives the agent a second chance.

---

## 3. Agency System Activation: NOT ACTIVE

### Finding: No roles, tradeoffs, or assignments are active in any B or C trial

**Evidence from wg state snapshots:**

- `agency/cache/` contains default starter roles, tradeoffs, and agents from `wg agency init` — but these are seeded by the adapter's `setup()` method, not by the trial agent.
- `agency/assignments/` is empty in all examined snapshots.
- `agency/evaluations/` is empty in all examined snapshots.
- No agent in any trial calls `wg_assign`, `wg_evaluate`, or `wg_agent_create` — these tools are **not exposed** in the adapter's tool list.
- All `graph.jsonl` entries show `"unplaced": true` — tasks are created without agent assignment.

**The adapter only exposes these wg tools to agents:**
- `wg_show`, `wg_list`, `wg_add`, `wg_done`, `wg_fail`, `wg_log`, `wg_artifact`, `wg_msg_send`, `wg_msg_read`

**Missing from the tool list:**
- `wg_assign`, `wg_agent_create`, `wg_evaluate`, `wg_agency_init`, `wg_evolve`

The agency system is completely invisible to trial agents. They cannot interact with it even if they wanted to.

### How agency works in Conditions D and E (for reference)

Conditions D and E (not yet run in full) DO bootstrap agency:
- **D:** Creates a "solver" agent with role=programmer, tradeoff=careful. Seeds identity in the prompt.
- **E:** Creates an "orchestrator" agent with role=architect, tradeoff=thorough.

But even D/E don't expose agency tools — the identity is injected into the prompt statically. The agent can't create new roles or assign identities to subtasks.

### Recommendations for agency integration

1. **Expose `wg_assign` tool** for Condition D/E. When an agent creates a subtask with `wg_add`, it should be able to assign an agent identity to it.

2. **Seed identity in root task assignment.** For Conditions D/E, the adapter should call `wg assign <root_task_id> <agent_hash>` so the graph.jsonl records the assignment.

3. **For B/C, agency is correctly absent.** These conditions test stigmergic coordination without identity — agency would be a confound.

---

## 4. Self-Verification Behavior (Autopoietic Loop Assessment)

### Quantitative breakdown (trials that called wg_done)

| Pattern | B-rerun (n=152) | C (n=143) |
|---------|-----------------|-----------|
| Test command before wg_done | 65 (42.8%) | 61 (42.7%) |
| Verify/check keyword in bash | 40 (26.3%) | 38 (26.6%) |
| Log completion message only | 31 (20.4%) | 32 (22.4%) |
| No verification at all | 16 (10.5%) | 12 (8.4%) |

### Qualitative patterns from transcript analysis

**Pattern A: Test → Fix → Re-test (closest to autopoietic)**
Example: `adaptive-rejection-sampler__NmaUXqD` (B-rerun, 12 turns)
- Turn 4: `bash("Rscript -e 'source(\"ars.R\"); test()'")`  → tests fail
- Turn 5: `write_file` — fixes numerical stability issues
- Turn 6: `bash("Rscript -e 'source(\"ars.R\"); test()'")`  → all tests pass
- Turn 7: `bash("ls -la /app/*.txt && head -5 ...")` — verifies output files
- Turn 8: `wg_log("Done: All 9 tests passed")`
- Turn 9: `wg_done`

This is the closest any trial gets to an autopoietic loop: implement → test → diagnose failure → fix → re-test → confirm → done. However, it's a single iteration, not a sustained cycle, and the "verification" is running the task's own tests (which the agent wrote), not an independent check.

**Pattern B: Implement → Run → Done (linear, no verification)**
Example: `fix-git__NJAXwgA` (B-rerun, 12 turns, no wg usage)
- Turns 0-9: Pure bash commands investigating git state, resolving merge conflict
- Turn 10: `bash("git log --oneline -5")` — checks result
- No wg_done, no structured verification, no iteration

The agent solves the problem linearly and stops when the last command succeeds. There's an implicit "check" (looking at git log output) but no explicit verification step.

**Pattern C: Implement → wg_done (skip verification entirely)**
Example: 16 B-rerun trials (10.5%) call `wg_done` with no preceding test, verify, or check command. The agent writes code and immediately marks done without executing it.

**Pattern D: Planning → Implement → Never finish**
Example: `chess-best-move__a7yCvWq` (B-rerun, 50 turns, no wg)
- All 50 turns are bash commands trying to parse a chess board image
- Agent never succeeds, hits the turn limit
- No wg_done, no wg_fail — just exhaustion

### How far from autopoietic?

The current behavior is **2-3 steps away** from a true autopoietic verification loop:

1. **What exists:** ~43% of trials run some form of test/check before completion. Some iterate on failures (the ARS example above). Agents log progress and signal completion.

2. **What's missing:**
   - **Independent verification.** Agents verify by running their own tests (which they wrote). There's no separation between implementer and verifier perspectives.
   - **Structured iteration protocol.** When tests fail, some agents fix and re-test, but most don't. There's no "retry up to N times" behavior — agents either fix it once or give up.
   - **Termination gating.** `wg_done` is not gated on verification success. 10.5% of agents call `wg_done` without any verification.
   - **Convergence detection.** No agent checks whether their fix actually changed the outcome. They don't compare "before vs after" — they just try again and hope.

### Recommendations for autopoietic verification loop

1. **Add a verification gate to wg_done.** In the adapter, when the agent calls `wg_done`, inject a follow-up message: "Before marking done: what verification did you run? What was the result? If you haven't verified, do so now." This forces the agent to either verify or explicitly skip.

2. **Prompt for structured verification.** Add to the Condition C prompt:
   ```
   ## Verification Protocol (MANDATORY)
   Before calling wg_done, you MUST:
   1. Run the task's test suite or verification command
   2. Log the result: wg_log("VERIFY: PASS — <evidence>") or wg_log("VERIFY: FAIL — <reason>")
   3. If FAIL: fix the issue, re-run verification, iterate up to 3 times
   4. Only call wg_done after a PASS verdict
   ```

3. **Implement verification in the adapter.** The adapter already has access to the Harbor environment. After the agent calls `wg_done`, the adapter could automatically run a verification command (e.g., the task's test suite) and log the result. If verification fails, re-inject the failure into the conversation and let the agent iterate.

4. **Condition D's prompt already does this.** The `build_condition_d_prompt` (L635-680) includes a "Core Loop: Attempt → Verify → Iterate → Declare" with explicit iteration limits. This should be tested at scale to see if it produces structured verification behavior.

---

## 5. Decomposition Behavior

### Quantitative summary

| Condition | Trials with wg_add | Rate | Avg subtasks/trial |
|-----------|-------------------|------|--------------------|
| B-rerun | 21/267 | 7.9% | 3.1 |
| C (full) | 15/267 | 5.6% | 3.9 |

### Decomposition examples

**Good decomposition:** `llm-inference-batching-scheduler__F9WMStj` (B-rerun)
- Created 4 sequential subtasks: Explore → Understand → Implement → Validate
- Marked subtasks done as completed
- Root task left open (agent ran out of turns before completing step 4)
- Graph showed proper pipeline structure

**Minimal decomposition:** `mcmc-sampling-stan__j7B3nZY` (B-rerun)
- Created 3 subtasks: Install RStan → Write model → Run MCMC
- All completed, root task marked done
- Proper sequential dependency handling

**Unnecessary decomposition:** `adaptive-rejection-sampler__DsvMush` (B-rerun)
- Created 1 subtask but then solved the task directly anyway
- Subtask left open — agent forgot about it

### Assessment

Decomposition is rare because most TB tasks are single-phase: implement one thing, verify it works. The 5.6-7.9% rate is appropriate — agents correctly identify that most tasks don't benefit from decomposition. The Condition C prompt's heuristic ("If 3+ distinct phases or might exhaust context") appears to be working as intended.

---

## 6. Prompting Effectiveness Assessment

### What works well in the current Condition C prompt

1. **Explicit wg_log template with task ID.** The `wg_log("{root_task_id}", "Starting: <plan>")` template drives 85%+ adoption of wg_log.
2. **Planning phase instruction.** The "analyze the task in ONE response" instruction produces structured planning turns in ~85% of trials.
3. **Decomposition heuristic.** The "3+ phases or might exhaust context" threshold correctly limits decomposition to complex tasks.
4. **Simple/complex classification.** Agents successfully classify ~85% of tasks as "simple (< 10 steps)" and skip decomposition.

### What doesn't work

1. **"Always do this" doesn't mean always.** 14-18% of agents ignore the "always" instructions entirely. The model treats "always" as "usually".
2. **No verification gating.** The prompt says to call wg_done when done but doesn't gate it on verification. 10.5% of agents call wg_done without any check.
3. **No iteration guidance.** When things fail, agents mostly don't iterate. The prompt doesn't teach "if your first attempt fails, diagnose and retry."
4. **Graph patterns section is unused.** The Condition B prompt includes pipeline/diamond/loop patterns, but no agent uses these. Condition C wisely omits them.

### Concrete changes to achieve target behaviors

#### (a) 100% wg quickstart on wake

**Adapter-level change (recommended):**
```python
# In WorkgraphAgent.run(), before the LLM loop:
if self.condition in ("B", "C"):
    await _exec_wg_cmd_host(wg_dir, wg_bin, 
        ["log", root_task_id, "Task started — agent initialized"])
```

**Prompt-level change (complementary):**
Replace "Always do this" with:
```
## MANDATORY First Action
Your FIRST tool call in this session MUST be:
  wg_log("{root_task_id}", "Starting: <your one-line plan>")
Do NOT call any other tool before this. This is not optional.
```

#### (b) Autopoietic verification loop as termination condition

**Adapter-level change:**
Intercept `wg_done` calls. Instead of immediately breaking the loop, inject a verification prompt:
```python
if fn_name == "wg_done" and fn_args.get("task_id") == root_task_id:
    # Don't break yet — ask for verification
    messages.append({
        "role": "user",
        "content": "VERIFICATION REQUIRED: Before this task can be marked done, "
                   "you must run a test or check command and report the result. "
                   "Call wg_log with 'VERIFY: PASS' or 'VERIFY: FAIL' before "
                   "calling wg_done again."
    })
    continue  # Don't break, let agent verify
```

**Prompt-level change:**
Add Condition D's verification protocol to the C prompt:
```
## Verification Loop (MANDATORY)
1. Implement your solution
2. Run verification (tests, diff, output check)
3. If FAIL: diagnose, fix, go to step 2 (max 3 iterations)
4. If PASS: wg_log("VERIFY: PASS — <evidence>"), then wg_done
5. NEVER call wg_done without a preceding PASS verdict
```

#### (c) Agency integration (role/tradeoff awareness)

**For Condition D/E (not B/C):**
1. Add `wg_assign` to CONDITION_D_TOOLS and CONDITION_E_TOOLS
2. In setup, assign the bootstrapped agent to the root task:
   ```python
   await _exec_wg_cmd_host(wg_dir, wg_bin, ["assign", root_task_id, "solver"])
   ```
3. When agent creates subtasks via `wg_add`, prompt it to also assign:
   ```
   When you create subtasks with wg_add, assign them:
     wg_assign("<subtask-id>", "solver")
   This tracks which identity is responsible for each piece of work.
   ```

**For B/C:** Agency should remain absent — it's a separate experimental variable.

---

## 7. Cross-Condition Comparison (Final Numbers)

| Metric | A (rerun) | B-rerun (=C) | C (full) |
|--------|-----------|--------------|----------|
| Trials | 269 | 270 | 266 |
| Pass (valid) | 52.8% (121/229) | 52.4% (121/231) | 51.1% (118/231) |
| Error rate | 14.9% | 14.4% | 13.2% |
| WG usage | 0% | 86.3% | 82.3% |
| WG done called | 0% | 56.3% | 53.8% |
| Decomposition | 0% | 7.8% | 5.6% |
| Test before done | N/A | 42.8% | 42.7% |
| Mean turns | 21.8 | 21.9 | 22.3 |
| Median turns | 17 | 17 | 18 |

**Pass rates are nearly identical.** With the B-rerun using ConditionCAgent (=C), we're comparing C vs C, so parity is expected. The pass rate difference from A (52.8%) to B/C (51-52%) is within noise.

**For a valid B vs C comparison, the original full-condition-b data (which used ConditionBAgent) must be used.** The early analysis found original B had 20% wg adoption and 38.1% pass rate (on partial data) vs C's 82% and ~51%.

---

## Appendix A: Detailed Trial Transcripts

### Transcript 1: Good WG + Verification (adaptive-rejection-sampler__NmaUXqD, B-rerun)
- **Result:** Fail (reward=0.0), 12 turns
- **Behavior:** Planning phase → install R → wg_log → implement → test fails → fix → re-test passes → verify output files → wg_log done → wg_done
- **WG calls:** wg_log ×2, wg_done ×1
- **Verification:** Yes — ran tests twice (fix cycle), checked output files
- **Note:** Despite good behavior pattern, verifier scored 0.0 — suggests implementation was correct by agent's tests but didn't meet TB task specification exactly

### Transcript 2: No WG, Linear Solve (fix-git__NJAXwgA, B-rerun)
- **Result:** Unknown (no wg_done), 12 turns
- **Behavior:** Pure bash — investigate git state, find orphaned commit, merge, resolve conflict, verify
- **WG calls:** 0
- **Verification:** Implicit — ran git log to check result
- **Root cause for no-wg:** Agent treated the task as straightforward git work, never considered using task management tools

### Transcript 3: Good WG, No Verification (build-pmars__6He23YC, C)
- **Result:** Pass (reward=1.0), 30 turns
- **Behavior:** wg_log immediately → long build process → iterative compilation fixes
- **WG calls:** wg_log ×1 (at start)
- **Verification:** No explicit verification step — agent relied on compilation success
- **Note:** Task was "build this software" — compilation itself is verification

### Transcript 4: No WG Despite C Prompt (cancel-async-tasks__5Drxper, C)
- **Result:** Fail (reward=0.0), 20 turns
- **Behavior:** Planning ("Simple task") → implement directly → test → fix → test → fix ...
- **WG calls:** 0
- **Verification:** Yes — ran tests multiple times, iterated on failures
- **Root cause for no-wg:** Planning phase classified task as "simple" and agent went directly to implementation, ignoring wg instructions entirely
- **Irony:** This trial shows good verification behavior (test → fix → re-test) but doesn't use wg to track it

### Transcript 5: Decomposition (llm-inference-batching-scheduler__F9WMStj, B-rerun)
- **Result:** Not completed (ran out of turns)
- **Behavior:** wg_log → created 4 subtasks → implemented steps 1-3, marked done → step 4 not reached
- **WG calls:** wg_log ×1, wg_add ×4, wg_done ×3 (subtasks)
- **Decomposition quality:** Good — proper sequential pipeline
- **Issue:** Agent ran out of turns before completing the full pipeline

### Transcript 6: Turn-limit exhaustion (chess-best-move__a7yCvWq, B-rerun)
- **Result:** Fail (reward=0.0), 50 turns (max)
- **Behavior:** 50 turns of bash commands trying to parse a chess board image
- **WG calls:** 0
- **Verification:** None — never got a working solution
- **Root cause for no-wg:** Task was too hard; agent spent all turns on implementation attempts

---

## Appendix B: Validation Checklist

- [x] All three result directories examined with per-trial breakdown
  - rerun-condition-a: 269 trials analyzed
  - rerun-condition-b: 270 trials analyzed  
  - full-condition-c: 266 trials analyzed
- [x] Adapter source code reviewed — B vs C differentiation confirmed
  - ConditionBAgent and ConditionCAgent are distinct classes with different prompts
  - BUT rerun-condition-b used ConditionCAgent (confounded)
- [x] At least 5 trial transcripts examined in detail (6 examined, see Appendix A)
- [x] Concrete, actionable recommendations produced (Section 6)
  - 100% wg quickstart: adapter-level auto-log + mandatory first-action prompt
  - Autopoietic loop: adapter-level wg_done interception + verification protocol prompt
  - Agency integration: expose wg_assign tool + prompt assignment guidance
- [x] Findings written to terminal-bench/analysis/agent-behavior-research.md
