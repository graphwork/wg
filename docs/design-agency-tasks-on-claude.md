# Migrate agency tasks (eval / flip / assign) onto the claude CLI path

Task: `migrate-agency-tasks` — investigate how `.evaluate-*`, `.flip-*`, and
`.assign-*` tasks dispatch today, then route them through the claude CLI
handler so they share the same auth / retry / logging behavior as worker
agents and stop silently failing when project config cascades a non-Anthropic
provider over them.

## 1. What does `executor=eval` actually do today?

There is **no `executor=eval` handler**. The `"eval"` string is a label
written into the agent registry (`.wg/service/registry.json`) and
metadata.json, nothing more — there is no branch in `handler_for_model`,
`plan_spawn`, or any executor switch that consumes it.

The actual code path:

1. The dispatcher loop in `src/commands/service/coordinator.rs` (around
   line 3698) detects tasks tagged `evaluation` / `flip` / `assignment` with
   a non-empty `exec` field and routes them through one of two **inline
   spawn** functions instead of the normal claude / native executor path:

   - `spawn_eval_inline` (lines ~2813–2998) — used for `.evaluate-*` AND
     `.flip-*` (flip tasks just have a different `exec` command like
     `wg evaluate run <id> --flip`).
   - `spawn_assign_inline` (lines ~3002–3169) — used for `.assign-*`.

2. Each inline spawn function:
   - Atomically claims the task (`Status::InProgress`, sets `assigned`).
   - Builds a bash script that invokes the task's `exec` field
     (`wg evaluate run …` / `wg assign … --auto`) with stdout/stderr
     redirected to the agent's `output.log`. On exit code 0 the script
     calls `wg done <id>`, on non-zero it calls `wg fail`.
   - Forks the script via `setsid` so it survives daemon restart.
   - Registers the spawned process in the agent registry with
     `executor="eval"` (or `"assign"`).

3. The `wg evaluate run` / `wg evaluate flip` / `wg assign --auto` commands
   in turn dispatch their actual LLM call via
   `workgraph::service::llm::run_lightweight_llm_call` (`src/service/llm.rs`).
   This function:
   - Resolves the model + provider for the dispatch role (Evaluator,
     FlipInference, FlipComparison, Assigner) via
     `Config::resolve_model_for_role`.
   - **Tries native API first** (Anthropic, OpenAI, OpenRouter, Local)
     when a provider is set.
   - **Falls back to claude CLI** (`claude --model X --print
     --output-format json --dangerously-skip-permissions`) on failure.
   - Returns `LlmCallResult { text, token_usage }`.

So the auth / retry / compaction / logging story for the eval path
*today* is:

- **Auth**: provider key from env (`ANTHROPIC_API_KEY` /
  `OPENROUTER_API_KEY` / etc.) → endpoint config → claude CLI's own auth
  (Anthropic OAuth flow / `~/.claude` token).
- **Retry**: only inside `run_evaluate` for JSON-extraction failures
  (3 attempts). No retry on transport failures.
- **Compaction**: none — this is a single-shot call with the full prompt.
- **Token logging**: tokens are reported via `wg evaluate record` (which
  emits `__WG_TOKENS__:{json}` lines that `wg done` parses).
- **Error surfacing**: stderr from the script is captured into
  `output.log` and the last 100 lines are echoed via `wg log` on failure
  before `wg fail` runs. But native-API errors inside
  `run_lightweight_llm_call` are **silently swallowed** — the
  `if let Ok(result) = call_*_native(…) { return Ok(result); }` pattern
  drops the error and falls through to claude CLI without surfacing why
  the native call failed. This is the bug behind today's outage.

## 2. Inputs / prompt shape / output format

- **Evaluator** (`.evaluate-*`): consumes the source task's transcript,
  description, validation criteria, agent identity, role, tradeoff, and
  artifact diff. Prompt is rendered by `render_evaluator_prompt`. Expects
  back JSON (`{score, dimensions, notes}`). Parsing failures retry up to
  3 times.
- **FlipInference** (`.flip-*`, first half): consumes the artifact diff
  + agent identity, asks the model to reconstruct the original intent
  prompt. Prompt rendered by `render_flip_inference_prompt`. Returns
  free-form text (the reconstructed prompt).
- **FlipComparison** (`.flip-*`, second half): consumes
  original-intent-prompt vs reconstructed-prompt, asks for a fidelity
  score `[0.0, 1.0]`. Prompt rendered by `render_flip_comparison_prompt`.
  Expects `{intent_fidelity, notes}` JSON.
- **Assigner** (`.assign-*`): consumes the source task description, the
  catalog of available agents (role + tradeoff per agent), and active
  task placements. Prompt rendered by `build_assignment_prompt`. Expects
  `{agent_hash, exec_mode, context_scope, placement?}` JSON.

All four roles produce **structured JSON** that the agency framework
parses. None of them require tool-use or multi-turn — every call is a
single prompt → single response.

## 3. Can claude CLI in print mode satisfy this?

**Yes**, and that's already the fallback path. `call_claude_cli` invokes
`claude --model X --print --output-format json
--dangerously-skip-permissions` with the prompt piped on stdin, parses
the `result` field as the model's text response, and extracts token
usage from the `usage` block. The agency framework's JSON extraction
runs over that text. No structured-output schema is needed at the CLI
level — the prompt instructs the model to emit JSON, and we parse it
ourselves.

The only requirement is that the model handed to claude CLI is one it
recognizes (`haiku` / `sonnet` / `opus` and Anthropic API IDs work; raw
strings like `anthropic/claude-sonnet-4-6` from openrouter prefixes do
not).

## 4. Are there callers that assume `executor=eval` specifically?

Yes — but all of them are display / observability, not routing:

- `src/service/registry.rs` — registry tracking & test fixtures use the
  literal string `"eval"`/`"assign"`.
- `src/commands/service/coordinator.rs` test assertions
  (`metadata["executor"] == "eval"`).
- `src/tui/viz_viewer/state.rs` — the TUI renders the executor field
  for display.

Nothing branches on the string to make a routing decision. We can rename
the registered label from `"eval"` / `"assign"` to `"claude"` without
changing dispatch behavior; we only need to update the test assertions.

## Decision

Implement **Option A — make agency tasks loudly run on claude CLI**:

1. Re-label the agent-registry executor field from `"eval"` / `"assign"`
   to `"claude"`. The model recorded becomes `claude:haiku` consistently.
   This aligns the observability with reality: the binary that ends up
   running the LLM call is `claude`, just like worker agents.

2. Force the claude CLI dispatch path in `run_lightweight_llm_call` for
   agency roles (Evaluator, FlipInference, FlipComparison, Assigner)
   when their resolved provider would have come **from cascade** rather
   than from an explicit `[models.<role>]` setting. Concretely: if the
   user has `coordinator.model = "openrouter:..."` set but never wrote
   `[models.evaluator].provider = "openrouter"`, the cascade is the
   accidental cause of openrouter showing up here, and we should ignore
   it and use claude CLI on the `claude:haiku` registry default.

3. Surface native-API errors loudly. When the native call fails before
   the claude CLI fallback, log a single-line warning so the daemon log
   shows *why* we fell back — no more silent swallowing.

4. Remove the `eval` / `assign` executor labels from the registry write
   path. Keep them as readable strings nowhere — the only string that
   describes "this is an inline one-shot agency call on claude CLI" is
   `"claude"`.

## Non-goals

- **Don't** change the inline-spawn architecture itself. `spawn_eval_inline`
  / `spawn_assign_inline` remain — a one-shot LLM call should not pay
  the overhead of a worker-style claude CLI session with tools enabled.
- **Don't** bump the model from `claude:haiku`. Agency tasks are kept
  cheap on purpose.
- **Don't** ship a deprecation alias for `executor=eval`. Nothing
  consumes the value as a routing signal, so there's no compatibility
  surface to preserve. Agent metadata.json from older runs still has
  `"executor": "eval"` literal but it's never read for routing.
