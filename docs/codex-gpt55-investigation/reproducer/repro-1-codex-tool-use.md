# Reproducer 1: Codex CLI Tool-Use Architecture and Lazy-Completion in workgraph

## Scope

This literature review captures what is known — from the workgraph source
tree, the upstream Codex CLI documentation, and the public model cards as of
2026 — about how the Codex CLI handles tool invocations, why the
`gpt-5.5`-era models tend toward "lazy completion" (text-only, no tool calls,
no committed deliverables), how `developer_instructions` partially close that
gap, and what architectural insights drop out of comparing the codex executor
path with the claude executor path inside workgraph.

Companion bibliography: `repro-1-refs.bib`.

## 1. How the Codex CLI handles tool invocations

The Codex CLI is OpenAI's open-source coding agent, built in Rust, that runs
locally and can read, edit, and execute code in the working directory
[openai-codex-cli, openai-codex-repo]. In workgraph it is invoked as a
non-interactive batch worker via the executor configured at
`src/service/executor.rs:1571-1588`:

```
codex exec --json --skip-git-repo-check --dangerously-bypass-approvals-and-sandbox
```

Tool invocations in this mode happen through three converging surfaces:

1. **Built-in shell tooling.** `codex exec` exposes a sandboxed shell that
   the model can drive via tool-call messages. With
   `--dangerously-bypass-approvals-and-sandbox` the approval policy is
   collapsed so the model never has to wait for a human "yes." This is the
   path workgraph relies on for `wg log`, `wg done`, and any file edits.
2. **Model Context Protocol (MCP).** Codex supports MCP servers for
   third-party tools and external context [openai-codex-cli]. workgraph does
   not currently register any MCP servers in the default executor config, so
   the model only sees the built-in shell.
3. **Skills.** Recent Codex releases ship a skills system — reusable bundles
   of instructions that the model can invoke explicitly or auto-select
   [openai-codex-cli]. workgraph's tier-classified guide
   (`src/commands/spawn/context.rs`) is conceptually adjacent but is injected
   as a static prompt, not as a Codex skill.

Architecturally, every tool round-trip in `codex exec --json` emits a JSON
event on stdout that the workgraph wrapper script never inspects. The
wrapper only watches the process exit code, which is decisive for the bug
described below.

## 2. Why gpt-5.5 tends toward lazy completion

`gpt-5.5` is part of the gpt-5.x family that includes `gpt-5.2-Codex`,
`gpt-5.4`, and `gpt-5.1-Codex-Max` [openai-gpt5-codex-max,
openai-introducing-gpt52-codex]. The investigation in
`docs/codex-gpt55-investigation/handler.md` and `fix-proposal.md` finds three
independent layers that push the model toward a text-only "I would do X, Y,
Z" answer rather than a tool-call sequence that produces committed
deliverables:

- **Layer A — knowledge tier gap.** `classify_model_tier`
  (`src/commands/spawn/context.rs:614-641`) does substring matching on the
  model string. `gpt-5.5` matches none of the substrings (`claude-sonnet`,
  `claude-opus`, `llama-3.1`, `qwen-2.5`, `deepseek`, `claude-haiku`,
  `minimax`) and falls through to `KnowledgeTier::Essential` (~8 KB), which
  omits the smoke-gate contract, the `## Validation` convention, and the
  "produce committed deliverables" doctrine. `claude-opus-4-7` matches
  `claude-opus` and lands in `Full` (~40 KB), so the two executors run with
  different rulebooks for the same task.
- **Layer B — Codex CLI lazy defaults.** The `gpt-5.5` model catalog
  hard-codes `default_verbosity = "low"` and a 10 000-token per-turn
  truncation limit. With no `developer_instructions` and RLHF that rewards
  "concise user-friendly answers," the model is heavily incentivized to
  produce a brief polished summary and stop — exactly the observed
  ~1.6 k-token "I would do…" output. Upstream issues #13950, #7247, #12225,
  and #19215 in `openai/codex` confirm this is a recurring gpt-5.x failure
  mode [openai-codex-repo, openai-codex-changelog].
- **Layer C — wrapper auto-`wg done` with no minimum-work gate.**
  `execution.rs:1398-1413` runs `wg done "$TASK_ID"` whenever
  `EXIT_CODE=0 && TASK_STATUS=in-progress`, with no check that the agent
  wrote a file, called `wg log`, recorded an artifact, or produced any diff.
  For `claude:opus` this is harmless because the model reliably calls shell
  tools first; for `codex:gpt-5.5` it converts a text-only bail into a false
  `done`.

A and B make the bail likely; C makes the bail indistinguishable from real
completion.

## 3. How `developer_instructions` address this

Codex exposes a top-level `developer_instructions` field that is injected at
the system-prompt level — the highest instruction-following weight available
to the CLI [openai-codex-cli, openai-codex-features]. Layer B of the
fix-proposal recommends overriding the default codex executor config with
three `-c` flags:

```
-c model_verbosity="high"
-c tool_output_token_limit=32000
-c developer_instructions="You are a non-interactive batch worker. You MUST
   complete the task by writing files to disk and creating at least one git
   commit before declaring done. A prose summary without file writes or
   commits is a task failure."
```

Each flag attacks a distinct catalog default. `model_verbosity=high`
counters the catalog `low` default that biases gpt-5.5 toward short
summaries. `tool_output_token_limit=32000` prevents shell output truncation,
which can otherwise cause the model to "give up" because it cannot see
partial progress. The `developer_instructions` block makes "must call tools"
non-optional from the model's perspective and pairs cleanly with the
workgraph-side completion contract injected through the tier guide. Because
existing user configs in `.wg/executors/codex.toml` override built-in
defaults, advanced users keep full control.

## 4. Key architectural insights

Three insights generalize beyond this specific bug:

1. **The completion gate is a structural component, not an
   implementation detail.** Both the claude and codex executors share a
   wrapper script that promotes a task to `done` purely on `EXIT_CODE == 0`.
   This is a single point of failure for any model that can produce
   exit-0 output without doing the requested work. The fix-proposal's
   minimum-work gate (Layer C) is the correct place to enforce the
   "produced at least one log entry, artifact, diff byte, or commit"
   invariant — a lighter-weight version of the kind of agentic verifier
   discussed in the long-horizon work literature [openai-gpt5-codex-max,
   openai-codex-max-system-card].
2. **Prompt-level doctrine and CLI-level levers are independent
   surfaces.** Layer A is fixed by promoting `gpt-5` and `gpt-4` to the
   Full tier (one line in `context.rs`). Layer B is fixed by `-c` overrides
   in `executor.rs`. Layer C is fixed by an inline shell branch in the
   wrapper. No fix subsumes the others; defense in depth is justified.
3. **Tool-availability signaling matters even when tools are present.**
   The codex executor arm of `build_inner_command` is a single code path
   that ignores `resolved_exec_mode` (`bare` / `light` / `full` / `resume`).
   For `light`-mode research tasks the claude executor passes
   `--allowedTools Bash(wg:*),Read,Glob,Grep`, an explicit tool allowlist;
   the codex executor passes only `--dangerously-bypass-approvals-and-sandbox`.
   When a model is on the fence between text and tools, the absence of an
   explicit tool-availability signal is itself an architectural bias.

## 5. References

See the companion file `repro-1-refs.bib` for full BibTeX entries covering
the OpenAI Codex CLI documentation, the GPT-5.x model cards, and the
upstream `openai/codex` repository.
