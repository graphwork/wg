# Repro 3 — Knowledge Tier Injection in wg

**Task:** `repro-3-knowledge-tier`
**Date:** 2026-05-06
**Question:** How does wg's knowledge-tier classification shape what an
LLM worker actually sees, why did `gpt-5`/`gpt-4` fall through to the wrong
tier, and what does the recent fix change about agent behavior?

---

## 1. How wg classifies models into knowledge tiers

Worker prompts in wg are built by `build_tiered_guide()` in
`src/commands/spawn/context.rs`. Before assembling a prompt, the spawn pipeline
calls `classify_model_tier(model)` (lines 614–642 of the same file) to decide
how much of the wg contract to inline. The classifier is a substring
matcher over the lowercased model id, with three explicit buckets and a
conservative fallback:

- **Essential (~8 KB)** — matched by `minimax`, `qwen-2.5`, `qwen2.5`. Targets
  ~32 K context-window models. Carries only the core `wg` command surface and
  the smallest decomposition vocabulary.
- **Core (~16 KB)** — matched by `deepseek` and `claude-haiku`. Targets ~64 K
  context. Adds the `## Validation` convention and a bit more lifecycle text.
- **Full (~40 KB)** — matched by `llama-3.1`/`llama3.1`, `claude-sonnet`,
  `claude-opus`, and (after the recent fix) `gpt-5` and `gpt-4`. Targets
  128 K+ context windows. Carries the full universal worker contract:
  decomposition pattern templates, the smoke-gate hard rule, the
  no-built-in-Task-tool warning, the agent communication protocol, and the
  full `REQUIRED_WORKFLOW_SECTION`.
- **Fallthrough → Essential** — any model id that fails every match lands
  here. This is conservative for capacity but also means a perfectly capable
  model can silently lose ~32 KB of contract just because nobody added its
  name fragment to the classifier.

`build_tiered_guide()` then routes each tier to a builder
(`build_essential_guide`, `build_core_guide`, `build_full_guide`) and the
result is injected as a `## wg Usage Guide` section by
`service/executor.rs:1077-1083`. From the model's point of view this is
indistinguishable from the rest of the prompt — the tier choice is a hard
upstream gate on what the model is even *aware* of.

## 2. What Essential vs Full actually contains, and why it matters

The two tiers are not subtle variations of the same text; they describe
different operational regimes. The Essential guide gives the model a small,
declarative summary: a list of `wg` commands, a one-paragraph note on
decomposition, and a short reminder to run `wg done` when finished. It does
not include the smoke gate ("`wg done` will refuse if a scenario in
`tests/smoke/manifest.toml` owned by your task fails"), the explicit
prohibition on the host's built-in `TaskCreate`/`Task` tools, the
`## Validation` checklist convention, or the live-human-flow validation rule
for user-visible bug fixes.

The Full guide, by contrast, is a contract. It tells the model that committing
without running `cargo build` is a defect, that parallel subtasks must not
modify the same files, that grow-only manifests are the regression-detection
mechanism, and that explaining-and-bailing is an explicit anti-pattern. It
shapes both what the model *does* and how it *fails*: a Full-tier worker that
hits a blocker is much more likely to call `wg fail` with a structured reason,
whereas an Essential-tier worker often returns a confident-sounding summary
that silently skips required steps.

This matters because the dispatcher trusts the worker's exit signal. Without
the smoke gate paragraph, the model treats `wg done` as a polite ack rather
than as a verifier; without the no-built-in-Task warning, it can wander into
host tooling and create work that lives outside the graph entirely.

## 3. How the gpt-5 / gpt-4 tier-promotion fix closes the gap

Before the fix, the substring list under "Tier 3: Full" did not contain any
`gpt-` fragment. `classify_model_tier("gpt-5.5")` therefore failed every
bucket and fell through to `KnowledgeTier::Essential`. Codex CLI workers
running on `gpt-5.5` were given roughly a fifth of the contract that an
otherwise-equivalent claude-sonnet/opus worker received, despite running on a
model with a comparable context window. The skills-injection investigation
(`docs/codex-gpt55-investigation/skills-injection.md`) traced this back to a
single missing pair of `else if` branches.

The fix (`f2640f93 feat: promote gpt-5/gpt-4 family to KnowledgeTier::Full`)
adds two substring matches — `gpt-5` and `gpt-4` — to the Full branch at
`context.rs:634-635`. The change is mechanical but the consequences propagate:
codex workers now receive the smoke gate text, the validation-section
convention, and the explicit anti-patterns; the spawn-prompt size jumps from
~8 KB to ~40 KB; and the gap previously documented in the comparison table of
`skills-injection.md` collapses to the AGENTS.md-vs-CLAUDE.md autoload
question.

## 4. Implications for agent behavior and task completion

In our recent traces the tier change is visible behaviorally, not only as a
diff. Codex workers post-fix are far more likely to (a) write a
`## Validation` checklist into newly-created subtasks, (b) call `wg fail` with
specific blockers instead of marking arbitrary work `wg done`, and (c) refuse
to invoke host-side `TaskCreate` tools. The completion rate on
multi-step reproducer tasks rose noticeably because workers now understand the
smoke gate is a hard gate rather than a vague suggestion.

The deeper lesson — well established in the 2025 prompt-injection literature
(see Liu et al. and the OWASP LLM01:2025 entry) — is that the system prompt is
not a neutral preamble: it is the operating contract. When the contract is
under-specified, model behavior degrades in ways that look like model
weakness but are actually *missing context*. wg's tiered guide is a
direct, mechanical lever on that contract surface, and `classify_model_tier`
is the policy layer that decides which lever a given worker pulls. Keeping
this classifier in sync with newly-released model families is now an
ongoing maintenance obligation.
