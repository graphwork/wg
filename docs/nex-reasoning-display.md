# Reasoning display in the nex CLI (`<think>` / collapse + toggle)

Reasoning models (Qwen3, DeepSeek R1, MiniMax, …) emit chain-of-thought
*reasoning* before their actual answer. nex receives that reasoning in
one of two ways while streaming:

1. **Inline `<think>…</think>` tags** in the assistant `content` channel
   (local servers — vLLM, SGLang, llama.cpp, Ollama — and any model that
   wasn't asked for a separate reasoning field).
2. **A separate reasoning channel** — the OpenAI-compatible `reasoning`
   field (OpenRouter) or its `reasoning_content` spelling (vLLM / SGLang
   / DeepSeek). nex requests this automatically for OpenRouter.

## Default: reasoning is collapsed

**By default nex does NOT print raw reasoning.** This is the documented
default — there is no flag you must pass to get it.

- While reasoning streams, the existing live prompt hint advances
  (`thinking… N tokens`).
- When reasoning ends, it collapses to a single dim line:

  ```
  ✓ thought for 412 tokens
  ```

- The answer then streams normally. Inline `<think>` tags are never shown
  and never leak into the chat transcript or the markdown re-render.

Tag detection is **streaming-safe**: a `<think>` or `</think>` tag split
across SSE deltas (e.g. `…<thi` then `nk>…`) is still recognized. See
`src/executor/native/think_filter.rs` (`ThinkSplitter`) and its unit +
fixture tests.

## Toggle: show the raw reasoning

Two equivalent ways to reveal raw reasoning:

- **REPL command** (live, per session):

  ```
  /think on      # reveal raw reasoning
  /think off     # collapse again (default)
  /think         # toggle the current state
  ```

  Synonyms: `on`/`show`/`raw`/`expand` and `off`/`hide`/`collapse`.

- **Environment default** (applies from the first turn):

  ```
  WG_NEX_THINK=1 wg nex          # start with reasoning shown
  ```

  Recognized "on" values: `1`, `true`, `on`, `show`, `raw`, `yes`,
  `expand`. Anything else (or unset) keeps the **collapsed default**.
  `/think` still toggles live afterwards.

When reasoning display is on, inline reasoning streams to the terminal as
it arrives and the field-channel reasoning is shown after the turn; both
are dimmed and still followed by the `✓ thought for N tokens` marker.

## Scope

This covers reasoning *display* only. Markdown rendering of the answer is
a separate concern (see `progressive-streaming-markdown`). Autonomous
task agents and piped output never show the marker or raw reasoning —
they only ever receive the clean answer text.
