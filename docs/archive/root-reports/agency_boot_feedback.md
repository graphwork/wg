# wg Feedback: Agency Needs Broader Role Coverage

**Date:** 2026-03-02
**Context:** Used wg to coordinate an NIH biosketch writing pipeline — web research, scientific writing (personal statement, honors, contributions to science), paper analysis, and validation/harmonization. Full DAG with fanout and fan-in.

## Problem

`wg agency init` seeds 8 roles but they're all software-oriented:

- Programmer, Reviewer, Architect, Documenter, Evaluator, Assigner, Evolver, Creator

The only worker role is "Careful Programmer." Everything else is agency infrastructure. So when I ran a pipeline with research tasks, scientific writing tasks, and a validation task, the assigner had no choice — it sent everything to "Careful Programmer." A web research task got the same role+tradeoff as a grant writing task as a literature analysis task. The evaluator scored them all fine (0.94 avg), but there was zero differentiation.

Real workloads aren't just programming. Even within software projects you need researchers, writers, analysts. My use case — grant writing — had zero programming. The entire agency was a mismatch.

## What Should Happen

The **Creator agent** should be detecting this gap and spawning new roles/agents on the fly. When a task comes in tagged `research` or `writing` and no role matches those skills, the creator should:

1. Notice that no existing role covers the task's domain
2. Propose a new role (e.g., "Researcher" with skills like web search, literature review, synthesis; or "Writer" with skills like drafting, revision, style matching, character-limit compliance)
3. Create an agent pairing that role with an appropriate tradeoff
4. Have the assigner route the task to the new agent

This should be done by a high-quality model (opus-tier). Role creation is a meta-cognitive task — understanding what kind of work a task requires and what agent profile would do it well. You don't want haiku deciding what new agent archetypes to mint.

## Suggested Starter Roles to Add to `agency init`

At minimum, the seed set should include non-programming roles:

| Role | Outcome | Skills |
|------|---------|--------|
| **Researcher** | Structured findings report | web search, literature review, source verification, synthesis |
| **Writer** | Polished prose document | drafting, revision, tone/style matching, constraint compliance (char limits, format rules) |
| **Analyst** | Recommendations with rationale | data gathering, comparative analysis, strategic reasoning |

These cover the vast majority of "knowledge work" tasks that aren't code. The current seed set assumes wg = software dev tool. It's not — it's a task coordination system, and the agency should reflect that.

## Concrete Suggestion: Creator-Driven Role Discovery

The creator pipeline (`creator -> evolver -> assigner`) exists but didn't fire during my run. It should be more aggressive:

- **Trigger:** When the assigner can't find a role with >50% skill overlap for a task's tags/description, it should invoke the creator before falling back to the default agent.
- **Creator prompt:** "Given this task description and the existing roles, is a new role needed? If so, define it."
- **Model:** Must be opus or equivalent. This is a judgment call about work taxonomy, not a rote operation.
- **Deferred vs. auto:** Could be deferred (human approves new role) for high-stakes, or auto-approved for low-risk additions. Config flag.

## Secondary Issue: `unknown` Role Tagging

In `wg agency stats`, all my actual work tasks showed up as `unknown` role. The assigner dispatched them but didn't tag them with a role, so the evolver has nothing to learn from. If the assigner routes to "Careful Programmer," it should at least tag the task with that role so the feedback loop works. Otherwise the synergy matrix stays empty for the Programmer column and the system can't learn.

## Summary

1. **Seed roles are too narrow** — add Researcher, Writer, Analyst to `agency init`
2. **Creator agent should fire on role mismatch** — don't just fall back to default, create what's needed
3. **Creator should use a high-quality model** — role design is meta-cognitive
4. **Fix unknown role tagging** — assigner should always tag dispatched tasks with the assigned role so the evolver can learn
