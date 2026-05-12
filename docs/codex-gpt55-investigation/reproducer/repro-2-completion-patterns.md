# LLM Agent Completion and Engagement Patterns (repro-2)

**Task:** `repro-2-agent-completion`
**Date:** 2026-05-06
**Companion data:** `repro-2-data.json`

This review synthesizes the wg fix-proposal analysis (`docs/codex-gpt55-investigation/fix-proposal.md`)
and skills-injection investigation (`docs/codex-gpt55-investigation/skills-injection.md`) with
public 2025 literature on tool-using agents, RLHF post-training, and agent evaluation.
The aim is a transferable explanation of *why* a non-interactive batch agent like
`codex:gpt-5.5` can exit cleanly with no committed deliverable, and what
engineering levers actually move the engagement-rate needle.

## 1. RLHF and the lazy-completion failure mode

Modern reasoning models are post-trained with a stack of techniques — supervised
fine-tuning, RLHF, and increasingly RLVR (reinforcement learning with verifiable
rewards) — that align the model to human preference signals. The dominant
human-preference signal during chat-style RLHF rewards *concise, polished,
user-friendly answers*. That is exactly the wrong objective for a non-interactive
batch worker, which must instead emit shell tool calls, write files, and produce
git commits before "stopping."

The mismatch shows up as the bug pattern documented for `gpt-5.5` in the
codex investigation: ~1.6 k tokens of well-structured prose ("I would do X, then
Y, then Z, and the deliverable would be Z'"), exit code 0, no files changed, no
commits, no `wg log` breadcrumbs. From the model's point of view this is a
maximally helpful response. From the orchestrator's point of view it is a
phantom completion. Upstream Codex CLI issues #13950, #7247, #12225, and #19215
trace this exact failure mode, all rooted in catalog defaults
(`default_verbosity = "low"`, `truncation_policy.limit = 10000`) that further
bias the model toward summary-shaped output.

The 2025 literature on tool-using agents converges on the same diagnosis:
quiz-style benchmarks reward right answers, but real agents are judged on whether
they *did* the work. New training corpora like TOUCAN (1.5M tool-agent
trajectories from real MCP environments) and verifiable-reward RL frameworks are
specifically designed to penalize "I would have called the tool" responses and
reward observable tool-call traces. The takeaway for orchestration engineers is
that you cannot rely on vanilla RLHF alignment to produce engagement; you must
either fine-tune on tool-use trajectories or shape the runtime to make
non-engagement structurally impossible.

## 2. The three-layer failure model

The wg fix-proposal identified three independent layers, each
sufficient on its own to produce the observed bail. This is a generalizable
mental model for any agent harness.

**Layer A — Knowledge tier gap (the rulebook the agent never sees).**
`classify_model_tier` (`src/commands/spawn/context.rs:614-641`) maps model
strings to knowledge tiers by substring. `gpt-5.5` matches none of the known
substrings and falls through to `KnowledgeTier::Essential` (~8 KB), which omits
the smoke-gate contract, the full `## Validation` section convention, the
"no built-in Task tool" warning, and the explicit completion contract. Meanwhile
`claude-opus-4-7` lands in `Full` (~40 KB) via the `claude-opus` substring. Two
executors thus run with different rulebooks on the same task description, and
codex never sees the doctrine that says "produce committed deliverables, not
summaries."

**Layer B — CLI defaults (the model's verbosity setting).**
The codex executor invoked `codex exec --json --skip-git-repo-check
--dangerously-bypass-approvals-and-sandbox` with no `-c` overrides. The
`gpt-5.5` model catalog hard-codes low verbosity and a tight per-turn token
limit, and there are no `developer_instructions`. Combined, the model has every
incentive to produce a brief polished summary and stop. This is the
highest-conviction *trigger* layer because it directly shapes generation.

**Layer C — Wrapper auto-done with no minimum-work gate.**
`execution.rs:1398-1413` ran `wg done "$TASK_ID"` whenever `EXIT_CODE=0 &&
TASK_STATUS=in-progress`. There was no check for "agent wrote a file," "agent
called `wg log`," "agent recorded an artifact," or even "output.log has more
than N bytes." For claude this rarely mattered because claude reliably calls
shell tools. For codex it converted a model-side bail into an indistinguishable
phantom completion at the orchestrator level.

Each layer is independently necessary to fully fix the bug: A gives the agent
the doctrine, B counters generation-time bias, C catches the residual case
where the model still bails. Single-layer fixes leave a residual failure rate.

## 3. Measurement approaches for engagement rate

Engagement rate is the empirical fraction of dispatches that produce a
committed deliverable: at least one file written or modified in the worktree,
at least one git commit on the worktree branch, and final task status `done`
(not `failed` and not `done` with zero diff). The fix-proposal's success metric
operationalizes this with a four-task reproducer pack spanning research /
mechanical edit / multi-file refactor / doc + code, run twice (baseline on
`main`, then post-fix), reporting both success ratio and a zero-false-done
check. The crucial detail is the secondary metric: `count(status=done AND
commits_ahead=0) == 0` — phantom completions must register as failures, not
successes, so the ratio is meaningful.

Because batch agent runs are noisy, robust measurement requires (i) a fixed
seed of diverse tasks, (ii) at least 3-5 trials per task to estimate variance,
(iii) holding model + executor + prompt constant across the A/B, and (iv)
logging the full output transcript so a human auditor can spot bails that
slipped past the wrapper gate. The 2025 agent-evaluation literature emphasizes
the same point: classic LLM benchmarks were built for quiz-takers, not agents,
and the only metrics that matter are ones tied to observable tool-use traces
and end-state side effects.

## 4. Best practices for agent completion

Distilled from the wg fix and corroborated by the 2025 RLHF /
agentic-RL literature:

- **Inject the contract at full fidelity.** Never let an agent run with a
  truncated rulebook. The cheapest fix is one line in the tier classifier
  (Fix #1 in the proposal); the broader principle is that agents should always
  see the smoke-gate contract, the validation-section convention, and the
  no-built-in-task-tool warning.
- **Override verbosity and instruction defaults at the CLI layer.** For
  Codex, that means `-c model_verbosity="high"`,
  `-c tool_output_token_limit=32000`, and a `-c developer_instructions=...`
  block that explicitly mandates file writes and at least one commit before
  declaring done.
- **Add a structural minimum-work gate.** The wrapper should refuse to
  promote a task to `done` when `LOG_COUNT < 1 AND ARTIFACT_COUNT < 1 AND
  DIFF_BYTES < 50 AND COMMITS_AHEAD < 1`. The conservative AND-of-four
  threshold tolerates legitimate no-op edits while catching the no-work bail.
- **Make the gate symmetrical across executors.** A claude bail (rate-limit
  truncation, etc.) is just as bad as a codex bail; the gate must catch both.
- **Treat false-done as a separate, dominant failure metric.** Report it
  alongside the headline success ratio so regression-testing makes phantom
  completions visible.
- **Decompose the fix; do not bundle.** Independent fixes in independent
  files give per-fix rollback. Bundling guarantees that one regression rolls
  back the whole improvement.

## 5. Sources

- [The State Of LLMs 2025: Progress, Progress, and Predictions](https://magazine.sebastianraschka.com/p/state-of-llms-2025)
- [From Self-Evolving Synthetic Data to Verifiable-Reward RL: Post-Training Multi-turn Interactive Tool-Using Agents](https://arxiv.org/html/2601.22607v1)
- [Fine-Tuning LLMs with Human Feedback (RLHF): Latest Techniques and Best Practices](https://medium.com/@meeran03/fine-tuning-llms-with-human-feedback-rlhf-latest-techniques-and-best-practices-3ed534cf9828)
- [Reinforcement Learning from Human Feedback (rlhfbook.com)](https://rlhfbook.com/book.pdf)
- [Reinforcement Learning from Human Feedback (arXiv 2504.12501)](https://arxiv.org/html/2504.12501v3)
- [Rethinking LLM Benchmarks for 2025: Why Agentic AI Needs a New Evaluation Standard](https://www.fluid.ai/blog/rethinking-llm-benchmarks-for-2025)
- Project source: `docs/codex-gpt55-investigation/fix-proposal.md`
- Project source: `docs/codex-gpt55-investigation/skills-injection.md`
