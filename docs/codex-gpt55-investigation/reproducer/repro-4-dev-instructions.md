# repro-4: Effectiveness of `developer_instructions` as an Agent-Behavior Enforcement Mechanism

**Topic:** Reviewing Fix #2 from `docs/codex-gpt55-investigation/fix-proposal.md` — injecting `developer_instructions`, `model_verbosity=high`, and `tool_output_token_limit=32000` into the codex executor's default args at `src/service/executor.rs:1571-1596`.

**Date:** 2026-05-06
**Model under study:** `codex:gpt-5.5` running through the `codex exec` CLI handler.

---

## 1. What `developer_instructions` is, and how it differs from user/system prompts

The Codex CLI stratifies prompt content across three roles: `system` (the model's hard-coded baseline doctrine, generally not user-editable), `developer` (an admin/integrator-level message bound to the conversation via configuration or `--config developer_instructions=...`), and `user` (the per-turn task content, including any `AGENTS.md` files which Codex injects as user-role messages). Per OpenAI's prompt-guidance documentation and the Codex configuration reference, `developer_instructions` is delivered as a message with `role="developer"`, and the GPT-5.x family has been post-trained to give developer-role messages priority over user-role messages: any conflict between an `AGENTS.md` direction and a `developer` directive resolves in favor of the developer message. Practically, a developer instruction sits between an immutable system prompt and the mutable user/agent messages, and it is the highest-leverage hook a CLI integrator can pull without forking the model.

That distinction matters for workgraph because the bug under investigation (`codex:gpt-5.5` declaring "done" with no files, no commits, ~1.6 k tokens of prose) is fundamentally a *role-priority* problem: workgraph's task description is delivered as a user-role message, but the model's catalog defaults (low verbosity, polite-summary RLHF) are baked deeper in the stack and outweigh user content.

## 2. Why workgraph uses it to enforce tool-calling in codex:gpt-5.5

Fix #2 in the proposal injects a developer-role mandate directly into the codex executor's default args:

> *"You are a non-interactive batch worker. You MUST complete the task by writing files to disk and creating at least one git commit before declaring done. A prose summary without file writes or commits is a task failure. Use shell tools (Read, Write, Edit, Bash) to do real work; do not describe work in the response."*

The text is calibrated to override exactly the failure mode observed: gpt-5.5's preference for a concise English summary over a tool-call sequence. By promoting the directive to the developer role, workgraph buys the strongest non-fork guarantee that the model will read the rule before it reads the task. This is in contrast to embedding the same rule inside the spawn prompt (user-role), which is what the `KnowledgeTier::Full` guide does — but tier doctrine alone proved insufficient on gpt-5.x because the model can rationalize "I summarized; that's an answer" against a user-role guideline more easily than against a developer-role mandate.

## 3. `model_verbosity=high` and `tool_output_token_limit=32000`

These two flags address adjacent failure modes that `developer_instructions` cannot solve alone.

- **`model_verbosity=high`** overrides the gpt-5.5 catalog default (`default_verbosity = "low"`), which biases the model toward terse summaries. Verbosity in Codex's config controls planning/explanation length, and "low" makes the model skip the explicit step-by-step "I will now run X" preamble that empirically precedes a tool call. Raising it to `high` increases the probability that the model both narrates its plan and then executes it.

- **`tool_output_token_limit=32000`** widens the per-turn truncation threshold (catalog default ~10 000) for shell/file output. When a build log or a large file read overflows the limit, Codex truncates and the model frequently interprets the truncation as "I cannot see the result, so I will give up and write a summary." Raising the limit removes that escape valve.

Together, the three flags target three independent failure mechanisms — instruction priority, narration verbosity, and observation truncation — which is why landing them as a unit produces a meaningfully larger improvement than any one alone.

## 4. Reliability across model families

`developer_instructions` is reliable on gpt-5.x and gpt-4.x families because OpenAI's RLHF explicitly trains for developer-role priority. Reliability degrades on:

- **Self-hosted / OSS models routed through `nex:` or OpenRouter** that do not implement Responses-API role semantics — the developer message may be flattened into a generic system message or prepended to the user turn, losing its priority.
- **Older Codex CLI versions and Windows Codex App 26.409.7971.0**, which has a known regression (issue openai/codex#18259) where project-local `developer_instructions` no longer override the user-level config.
- **Non-Codex executors** (claude CLI, in-process nex handler) where the flag has no analogue and is silently ignored — workgraph correctly applies these flags only to the `codex` executor branch.

## 5. Limitations and failure modes

Even in the happy path, `developer_instructions` is best-effort: users have reported (Codex GitHub discussions) that logs sometimes show no trace of the developer message being applied, and the override mechanism is not strictly enforceable from the model side. Because the mechanism is *suggestive* rather than mechanical, the workgraph design correctly pairs it with Fix #3's wrapper-level minimum-work gate, which is the only purely structural defense — if the model still bails despite all three flags, the wrapper refuses to auto-`wg done` and the task is failed for retry/escalation. `developer_instructions` raises the *probability* of correct behavior; the wrapper gate enforces the *consequence* of incorrect behavior. Neither is sufficient alone, which is why Fix #2 is "defense in depth" rather than the primary fix.

## Sources

- [Prompt guidance | OpenAI API](https://developers.openai.com/api/docs/guides/prompt-guidance)
- [Codex Prompting Guide](https://developers.openai.com/cookbook/examples/gpt-5/codex_prompting_guide)
- [Configuration Reference – Codex](https://developers.openai.com/codex/config-reference)
- [Custom instructions with AGENTS.md – Codex](https://developers.openai.com/codex/guides/agents-md)
- [openai/codex Issue #18259 — developer_instructions override regression](https://github.com/openai/codex/issues/18259)
- [openai/codex Discussion #7296 — Custom system prompt tip](https://github.com/openai/codex/discussions/7296)
