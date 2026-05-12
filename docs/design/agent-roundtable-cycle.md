# Agent Roundtable: Collaborative Website Review Cycle

*Design document for a recurring multi-agent review-and-improve cycle*

---

## 1. Overview

The **Agent Roundtable** is a cyclic workflow where 3–4 specialized reviewer agents collaboratively identify, prioritize, and implement improvements to the wg website. It leverages wg's structural cycle support (`--max-iterations`, `--converged`) and scatter-gather topology to produce iterative refinement without requiring direct agent-to-agent communication.

### Design Principles

1. **Stigmergic coordination** — agents communicate through artifacts in the graph, not direct messaging. The graph *is* the conversation (per R7/R10 in the deliberation synthesis).
2. **Structured convergence** — each iteration must produce measurably fewer issues than the last, with a formal convergence signal.
3. **Human-in-the-loop option** — a configurable checkpoint before implementation lets the user veto or reprioritize.
4. **Same files = sequential edges** — the implementation phase is a single task to avoid parallel file conflicts.

---

## 2. Cycle Structure

### 2.1 Topology

The roundtable is a **scatter-gather wrapped in a structural cycle**:

```
                    ┌─── reviewer-ux ────────┐
                    │                        │
roundtable-discuss ─┼─── reviewer-code ──────┼─── roundtable-synthesize
        ▲           │                        │          │
        │           └─── reviewer-content ───┘          │
        │                                               ▼
        │                                    roundtable-implement
        │                                               │
        │                                               ▼
        └──────────────────────────────────── roundtable-verify
                         (back-edge)
```

All 7 tasks are members of a single structural cycle. `roundtable-discuss` is the **cycle header** with `--max-iterations 5`.

### 2.2 Phase Description

| Phase | Task(s) | Parallelism | Role | Purpose |
|-------|---------|-------------|------|---------|
| **Discuss** | `roundtable-discuss` | Single | Architect | Set iteration context: what changed last iteration, what to focus on. On iteration 0, performs initial website audit. |
| **Review** | `reviewer-ux`, `reviewer-code`, `reviewer-content` | Parallel (scatter) | Specialized reviewers | Independent review from different perspectives. Each produces a structured review artifact. |
| **Synthesize** | `roundtable-synthesize` | Single (gather) | Architect | Read all review artifacts, resolve conflicts, produce a prioritized action plan. |
| **Implement** | `roundtable-implement` | Single | Programmer | Execute the top-priority items from the action plan. Single agent to avoid file conflicts. |
| **Verify** | `roundtable-verify` | Single | Reviewer | Compare implementation against action plan. Determine convergence. |

### 2.3 Iteration Lifecycle

On each cycle iteration:

1. `roundtable-discuss` opens (or re-opens). The agent reads the previous iteration's verify report (if any) and sets the focus for this iteration.
2. The three reviewer tasks open in parallel once discuss completes.
3. `roundtable-synthesize` opens once all three reviewers complete. Produces a ranked action plan.
4. `roundtable-implement` opens. The implementer works through as many action items as feasible.
5. `roundtable-verify` opens. The verifier checks implementation quality and decides:
   - **Continue**: `wg done roundtable-verify` — cycle resets, next iteration begins.
   - **Converge**: `wg done roundtable-verify --converged` — cycle terminates.

---

## 3. Communication Protocol

### 3.1 Primary Channel: Artifacts (Stigmergic)

Each task produces a structured artifact at a predictable path. Downstream tasks read these artifacts to coordinate.

| Task | Artifact Path | Format |
|------|--------------|--------|
| `roundtable-discuss` | `docs/roundtable/iteration-{N}-brief.md` | Iteration brief: focus areas, constraints, prior context |
| `reviewer-ux` | `docs/roundtable/iteration-{N}-review-ux.md` | UX review findings |
| `reviewer-code` | `docs/roundtable/iteration-{N}-review-code.md` | Code quality/perf/a11y review |
| `reviewer-content` | `docs/roundtable/iteration-{N}-review-content.md` | Copy/messaging/IA review |
| `roundtable-synthesize` | `docs/roundtable/iteration-{N}-action-plan.md` | Prioritized action plan |
| `roundtable-implement` | `docs/roundtable/iteration-{N}-changelog.md` | What was implemented + commit refs |
| `roundtable-verify` | `docs/roundtable/iteration-{N}-verify-report.md` | Verification results + convergence decision |

Each artifact is registered with `wg artifact <task-id> <path>` so the graph tracks provenance.

The iteration number `{N}` comes from `wg show <task-id>` → `loop_iteration`.

### 3.2 Review Artifact Schema

Each reviewer produces a markdown document with this structure:

```markdown
# Website Review: {Perspective} — Iteration {N}

## Summary
One-paragraph assessment of current state from this perspective.

## Findings

### Critical (must fix)
1. **{Issue title}** — {description}. File: `{path}:{line}`. Suggested fix: {suggestion}.

### Important (should fix)
1. ...

### Minor (nice to have)
1. ...

## Scores
- Overall quality: {1-5}
- Improvement since last iteration: {-2 to +2} (0 = no change)
```

The structured format enables the synthesizer to parse and cross-reference programmatically.

### 3.3 Action Plan Schema

The synthesizer produces:

```markdown
# Action Plan — Iteration {N}

## Priority Rankings

Items are ranked by consensus weight: number of reviewers flagging × severity tier.

| Rank | Item | Flagged By | Severity | Consensus Weight |
|------|------|-----------|----------|-----------------|
| 1 | ... | UX, Code | Critical | 6 |
| 2 | ... | Content | Critical | 3 |
| 3 | ... | UX, Content | Important | 4 |

## Recommended Actions (ordered)
1. {Action}: {what to do}, {which files}, {acceptance criteria}
2. ...

## Deferred to Next Iteration
- {Item}: {reason for deferral}

## Conflicts Resolved
- {Reviewer A} suggested X, {Reviewer B} suggested Y → chose {resolution} because {reason}
```

### 3.4 Secondary Channel: `wg msg` (Exception Only)

`wg msg` is reserved for **urgent, exceptional communication** — not routine coordination. Examples:
- User sends `wg msg roundtable-implement "STOP — do not touch index.html, it's being deployed"` with `--priority urgent`
- Verifier sends `wg msg roundtable-discuss "CRITICAL: iteration 2 broke the build, rollback needed"` before the next iteration starts

Routine information flows through artifacts, not messages.

---

## 4. Consensus Mechanism

### 4.1 Weighted Priority Scoring

Consensus is **quantitative, not deliberative**. There is no debate phase — instead, independent reviews are merged by a scoring algorithm:

```
consensus_weight = count(reviewers_flagging) × severity_multiplier

where severity_multiplier:
  critical = 3
  important = 2
  minor = 1
```

An issue flagged as "critical" by 2 reviewers scores `2 × 3 = 6`. An issue flagged as "minor" by 1 reviewer scores `1 × 1 = 1`.

The synthesizer ranks all issues by consensus weight and selects the top items that fit within one iteration's implementation budget (typically 5–10 changes).

### 4.2 Conflict Resolution

When reviewers disagree (e.g., UX wants larger fonts, Code wants smaller bundles):
1. The synthesizer documents the conflict in the action plan.
2. If both are actionable independently, both are included.
3. If mutually exclusive, the higher consensus-weight item wins.
4. True deadlocks escalate via `wg msg` to the user for resolution.

### 4.3 Why Not Deliberation?

A deliberation phase (agents debating proposals) was considered and rejected:
- **Cost**: Each deliberation round costs an additional LLM call per agent, with marginal value.
- **Convergence risk**: Agent debates can cycle without converging (each agent optimizes for its own perspective).
- **wg alignment**: The scatter-gather pattern with quantitative synthesis is already wg's strongest multi-agent pattern (per the organizational patterns doc). Adding deliberation introduces agent-to-agent coupling that fights the stigmergic model.

If future iterations reveal that the quantitative approach misses important nuance, a lightweight deliberation step could be added: each reviewer reads the draft action plan and posts a single `wg msg` with objections. The synthesizer incorporates these before finalizing. This is **not included in v1**.

---

## 5. Convergence Criteria

### 5.1 Automatic Convergence (`--converged`)

The verifier signals convergence when **all** of the following hold:

1. **No critical findings** in any reviewer's report for this iteration.
2. **Overall quality scores** from all reviewers are ≥ 4/5.
3. **Improvement-since-last-iteration** scores are all ≤ 0 (no significant improvement possible = work is done).
4. **All action plan items** were either implemented or explicitly deferred as out-of-scope.

When these criteria are met, the verifier runs:
```bash
wg done roundtable-verify --converged
```

### 5.2 Safety Net: `--max-iterations 5`

If convergence is never signaled, the cycle hard-stops after 5 iterations. This is a safety net, not the expected path. In practice:
- Iteration 0: Initial audit, many findings. ~15 action items.
- Iteration 1: Major issues resolved. ~8 action items.
- Iteration 2: Polish and refinement. ~3 action items.
- Iteration 3: Convergence likely. Verifier signals `--converged`.

Five iterations is generous — most reviews should converge in 2–3.

### 5.3 Failure Handling

If any task in the cycle fails:
- Cycle automatically restarts from the header (default behavior with `--max-failure-restarts 3`).
- The failing task's error is logged and visible to the restarted discuss phase.
- After 3 failure restarts, the cycle stops and escalates to the user.

---

## 6. Human Checkpoint (Optional)

### 6.1 Design

A human checkpoint between synthesis and implementation is **recommended but optional**. When enabled, the synthesizer sends a message to the user after producing the action plan:

```bash
wg msg roundtable-implement "ACTION PLAN READY for iteration {N}. Review docs/roundtable/iteration-{N}-action-plan.md before I proceed." --from synthesizer --priority urgent
```

The implementer checks for messages before starting (per the standard agent workflow). If the user has responded with modifications, the implementer incorporates them.

### 6.2 When to Enable

- **Enable** for production websites, high-stakes changes, or early iterations where trust hasn't been established.
- **Disable** for internal/staging sites or when the team is confident in the reviewer agents' judgment.

This is a social convention, not a graph-level gate. A hard gate (pausing the implement task) could be added later if needed, but would require coordinator-level support for "approve before dispatch."

---

## 7. Agent Roles

| Task | Recommended Role | Recommended Tradeoff | Model Tier |
|------|-----------------|---------------------|------------|
| `roundtable-discuss` | Architect | Thorough | sonnet |
| `reviewer-ux` | Reviewer (UX skill) | Thorough | sonnet |
| `reviewer-code` | Reviewer (Code skill) | Careful | sonnet |
| `reviewer-content` | Analyst (Content skill) | Thorough | sonnet |
| `roundtable-synthesize` | Architect | Thorough | opus |
| `roundtable-implement` | Programmer | Careful | sonnet |
| `roundtable-verify` | Reviewer | Careful | sonnet |

The synthesizer uses opus because it must reason across multiple conflicting inputs — the most cognitively demanding task in the cycle.

---

## 8. Example Setup Commands

### 8.1 Create the Cycle

```bash
# Create the cycle header (discuss phase)
wg add "Roundtable: discuss and set review focus" \
  --id roundtable-discuss \
  --max-iterations 5 \
  --max-failure-restarts 3 \
  --model sonnet \
  -d "## Description
Review the current website state. On iteration 0, perform a full audit.
On subsequent iterations, read the previous verify report and set focus areas.
Produce an iteration brief artifact.

## Artifacts
- docs/roundtable/iteration-{N}-brief.md

## Validation
- [ ] Iteration brief produced with clear focus areas
- [ ] Previous iteration's verify report acknowledged (if iteration > 0)
- [ ] Brief registered as artifact"

# Create parallel reviewer tasks (scatter)
wg add "Roundtable: UX review" \
  --id reviewer-ux \
  --after roundtable-discuss \
  --model sonnet \
  --exec-mode light \
  -d "## Description
Review the website from a UX perspective: navigation, visual hierarchy,
responsiveness, interaction patterns, accessibility.
Read the iteration brief from roundtable-discuss for focus areas.

## Artifacts
- docs/roundtable/iteration-{N}-review-ux.md (use review schema)

## Validation
- [ ] Review follows the structured schema (Summary, Findings by severity, Scores)
- [ ] Scores provided (quality 1-5, improvement -2 to +2)"

wg add "Roundtable: code quality review" \
  --id reviewer-code \
  --after roundtable-discuss \
  --model sonnet \
  --exec-mode light \
  -d "## Description
Review website code quality: HTML semantics, CSS efficiency, JS performance,
asset optimization, SEO meta tags, accessibility attributes, build output.
Read the iteration brief for focus areas.

## Artifacts
- docs/roundtable/iteration-{N}-review-code.md (use review schema)

## Validation
- [ ] Review follows the structured schema
- [ ] Specific file:line references for each finding"

wg add "Roundtable: content and messaging review" \
  --id reviewer-content \
  --after roundtable-discuss \
  --model sonnet \
  --exec-mode light \
  -d "## Description
Review website content: copy clarity, value proposition, information architecture,
tone consistency, CTAs, documentation links, SEO content.
Read the iteration brief for focus areas.

## Artifacts
- docs/roundtable/iteration-{N}-review-content.md (use review schema)

## Validation
- [ ] Review follows the structured schema
- [ ] Content suggestions include specific replacement text"

# Create synthesizer (gather)
wg add "Roundtable: synthesize reviews into action plan" \
  --id roundtable-synthesize \
  --after reviewer-ux,reviewer-code,reviewer-content \
  --model opus \
  -d "## Description
Read all three review artifacts. Apply weighted priority scoring:
  consensus_weight = count(reviewers_flagging) × severity_multiplier
  (critical=3, important=2, minor=1)
Produce a ranked action plan. Resolve conflicts. Select top items for implementation.
Optionally notify user for review before implementation proceeds.

## Artifacts
- docs/roundtable/iteration-{N}-action-plan.md

## Validation
- [ ] All three reviews read and cross-referenced
- [ ] Priority rankings computed with consensus weights
- [ ] Conflicts documented with resolution rationale
- [ ] Action plan has concrete, implementable items"

# Create implementer
wg add "Roundtable: implement agreed changes" \
  --id roundtable-implement \
  --after roundtable-synthesize \
  --model sonnet \
  -d "## Description
Execute the action plan produced by the synthesizer. Work through items in
priority order. Commit each logical change separately. Stop when all items
are done or time budget is exhausted.

## Artifacts
- docs/roundtable/iteration-{N}-changelog.md

## Validation
- [ ] Action plan items implemented in priority order
- [ ] Each change committed with descriptive message
- [ ] cargo build passes (if applicable)
- [ ] Changelog artifact records what was done + commit hashes"

# Create verifier (with back-edge to header)
wg add "Roundtable: verify implementation and assess convergence" \
  --id roundtable-verify \
  --after roundtable-implement \
  -d "## Description
Compare implementation against the action plan. Check that changes are correct
and don't introduce regressions. Assess convergence:

Signal --converged when ALL of:
- No critical findings from any reviewer this iteration
- All reviewer quality scores >= 4/5
- Improvement scores <= 0 (diminishing returns)
- All action items addressed or explicitly deferred

Otherwise, complete normally to trigger the next iteration.

## Artifacts
- docs/roundtable/iteration-{N}-verify-report.md

## Validation
- [ ] Each action plan item verified as implemented or documented why not
- [ ] Convergence criteria explicitly evaluated
- [ ] Verify report produced with clear pass/fail per item"

# Close the cycle: add back-edge from verify to discuss
# (This requires adding roundtable-verify as a dependency of roundtable-discuss)
# Use wg edit or the appropriate command to add the back-edge:
wg dep add roundtable-discuss roundtable-verify
```

### 8.2 Create the Artifact Directory

```bash
mkdir -p docs/roundtable
```

### 8.3 Start the Service

```bash
wg service start --max-agents 4
# The coordinator will dispatch roundtable-discuss first,
# then the three reviewers in parallel, then synthesize, etc.
```

### 8.4 Monitor Progress

```bash
# Watch the cycle iterate
wg watch roundtable-discuss

# Check current iteration
wg show roundtable-discuss  # look for loop_iteration

# Read the latest action plan
cat docs/roundtable/iteration-$(wg show roundtable-discuss --json | jq -r '.loop_iteration // 0')-action-plan.md

# Send a message to influence the next iteration
wg msg roundtable-discuss "Focus on mobile responsiveness this iteration" --from user
```

---

## 9. Variations and Extensions

### 9.1 Fewer Reviewers

For simpler sites or tighter budgets, reduce to 2 reviewers (e.g., UX + Code) or even 1 comprehensive reviewer. The cycle structure is the same — just fewer scatter tasks.

### 9.2 Specialized Iterations

The discuss phase can steer focus per iteration:
- Iteration 0: Full audit (broad)
- Iteration 1: Fix critical issues (narrow)
- Iteration 2: Polish and UX (focused)
- Iteration 3: Performance and SEO (focused)

This is controlled by the discuss agent's iteration brief, not by graph structure.

### 9.3 Cross-Site Roundtable

For multiple websites (e.g., docs site + marketing site), create separate roundtable cycles per site. A meta-task can synthesize cross-site findings after both cycles converge.

### 9.4 Reusable Function

Once proven, the roundtable pattern should be extracted as a `wg func`:

```bash
wg func create roundtable \
  --input "site_path:Path to the website source" \
  --input "reviewer_count:Number of parallel reviewers (2-4)" \
  --input "max_iterations:Maximum review cycles (default 5)"
```

This allows one-command instantiation for future projects.

---

## 10. Cost and Time Estimates

Per iteration (assuming sonnet for most tasks, opus for synthesis):

| Task | Est. Tokens (in/out) | Model | Est. Cost |
|------|---------------------|-------|-----------|
| discuss | 20k/2k | sonnet | ~$0.07 |
| reviewer-ux | 40k/3k | sonnet | ~$0.14 |
| reviewer-code | 40k/3k | sonnet | ~$0.14 |
| reviewer-content | 40k/3k | sonnet | ~$0.14 |
| synthesize | 30k/4k | opus | ~$0.60 |
| implement | 60k/8k | sonnet | ~$0.25 |
| verify | 30k/3k | sonnet | ~$0.11 |
| **Total per iteration** | | | **~$1.45** |
| **3 iterations (typical)** | | | **~$4.35** |
| **5 iterations (max)** | | | **~$7.25** |

These are rough estimates. Actual costs depend on website complexity and context window usage.

---

## 11. Open Questions for Implementation

1. **`wg dep add` command**: The back-edge creation assumes a `wg dep add` command exists. If not, the implementer will need to add the dependency via graph editing or extend the CLI.

2. **Iteration number in artifact paths**: Agents need to read their `loop_iteration` from `wg show` and use it in artifact filenames. This convention should be documented in the agent skill.

3. **Artifact directory git hygiene**: The `docs/roundtable/` directory will accumulate artifacts across iterations. Should old iterations be pruned, or kept as a review trail?

4. **Reviewer agent creation**: The UX, Code, and Content reviewer roles may not exist in the agency yet. They'll need to be created with appropriate skills and desired outcomes before the first roundtable.

---

## 12. Summary

The Agent Roundtable is a **scatter-gather cycle** that:
- Uses **3 parallel reviewers** for diverse perspectives (scatter)
- **Synthesizes** findings with quantitative consensus scoring (gather)
- **Implements** prioritized changes (pipeline)
- **Verifies** and decides convergence (cycle control)
- Communicates via **structured artifacts** (stigmergic)
- Stops via **`--converged`** or **`--max-iterations 5`** (bounded)

It maps cleanly onto wg's existing primitives: `--after` edges for topology, `--max-iterations` for cycle bounds, `--converged` for early termination, `wg artifact` for provenance, and the agency system for role-based dispatch.
