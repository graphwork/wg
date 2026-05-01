# WGNEX executor improvements: toward self-bootstrapping

Status: **proposed 2026-04-18**, revised after session reflection.
Supersedes no prior doc. Sequel to `nex-as-coordinator.md` (shipped
2026-04-18), which established one architectural shape for
coordinators — UUID-keyed sessions under `chat/<uuid>/`, aliases,
inotify, `wg session` CLI.

## Goal

**Make the native executor (`wg nex`) strong enough that workgraph
can dispatch agents to work on itself without a human in the loop.**

Today, `executor=claude` via `claude --print` is the billing-
compatible default for users on Claude Code subscriptions, and
it works well enough that most user-level work gets done through
it. `wg nex --chat <ref>` exists and works end-to-end for the
openrouter / oai-compat / SGLang / direct-API-key paths, but it's
not yet at the reliability bar where you'd dispatch long
self-improvement work through it without supervision. This doc is
the plan to close that gap.

Every improvement below is judged against one question: *does this
make the executor more trustworthy for self-dispatched work?* If a
proposal doesn't move that needle, it's on the deferred list.

## Ranking

Listed by strategic impact on the self-bootstrap goal, not by
implementation order (though implementation order follows it here).

1. **MCP client** — the single biggest unlock.
2. **LLM-backed compaction** — quality floor on long runs.
3. **Tokenizer-aware token counts** — pressure detection accuracy.
4. **External-executor class (design → implementation)** — so
   claude, codex, amplifier, and future peers plug in uniformly.

Execution style: one commit per step, each pushed on its own, pause
for verification between them. No bundling.

---

## Step 1 — MCP client

**Goal.** Ship a stdio MCP client so WGNEX can use the growing
ecosystem of third-party tool servers (filesystem, sequential-
thinking, github, sentry, linear, browser, fetch, sqlite, memory,
…) without each one needing a hand-rolled Rust wrapper.

**Why this first.** Right now every new integration means writing
a tool in `src/executor/native/tools/` in Rust. That's the reason
our tool set is frozen at what we've built — the marginal cost of
adding a new capability is high. With MCP, the marginal cost of
adding a new capability is "write a JSON config stanza pointing at
an MCP server." Every serious peer executor has MCP (Claude Code,
OpenCode, Goose, Amplifier, Cline, Nanobot). Without it, WGNEX is
on a divergent trajectory.

For self-bootstrap specifically: workgraph tasks that need to
search GitHub, query Linear, write to Slack, read Sentry errors,
etc. — all of these become reachable the moment MCP lands, without
us writing Rust code for each.

**Sub-phases.**

- **1a.** stdio transport, `tools/list` + `tools/call`, schema
  translation, namespaced tool IDs (`<server>__<tool>`), config-
  driven server launch, supervised lifecycle with rate-limited
  crash restart. **This is what "Step 1 done" means.**
- **1b.** resources + prompts. `resources/list/read` as a
  first-class `mcp_read` tool. `prompts/list/get` as slash commands.
- **1c.** SSE and WebSocket transports. Deferred; most valuable
  MCP servers today are stdio.

**Scope (1a only).**

New module `src/executor/native/mcp/`:

```
mcp/
├── mod.rs          — pub API: McpManager, McpTool
├── transport.rs    — StdioTransport + trait Transport
├── client.rs       — JsonRpcClient (request/response/notification)
├── supervisor.rs   — server lifecycle: spawn, keepalive, restart
├── schema.rs       — MCP JSON Schema → Anthropic ToolDefinition
└── registry.rs     — merges MCP tools into ToolRegistry
```

Config in `.wg/config.toml`:

```toml
[[mcp.servers]]
name = "filesystem"
command = "npx"
args = ["-y", "@modelcontextprotocol/server-filesystem", "/workspace"]
enabled = true

[[mcp.servers]]
name = "sequential-thinking"
command = "npx"
args = ["-y", "@modelcontextprotocol/server-sequential-thinking"]
enabled = true
```

**Acceptance.** `wg nex --chat <ref>` with an enabled filesystem
MCP server exposes `filesystem__read_file` in its tool list and
the agent can call it successfully. Config-disabled servers are
not spawned. A crashed server restarts up to 3 times per 10min,
then stays down with a logged error.

**Scope estimate.** ~1000-1500 LOC, new module. Probably 1-2
working days.

---

## Step 2 — LLM-backed compaction

**Goal.** Replace the local heuristic summarizer in
`resume.rs:289` with a real LLM call that writes a structured
9-section summary into the `JournalEntryKind::Compaction` entry.
Close the comment at `resume.rs:287-289` that's been explicitly
asking for an "external process" to do this since day one.

**Why.** The current `summarize_messages` walks the message list,
grabs tool-call names and the first 200 chars of text blocks, joins
them. The resulting summary is enough to give replay *something*
post-compaction, but it doesn't preserve the *reasoning* or
*decisions* the agent made. OpenHands' `LLMSummarizingCondenser`
reports ~2× cost reduction *and* higher task continuity
post-compact — we want both. Especially matters for long
self-dispatched runs where one compaction event can erase
everything the agent knows about what it was doing.

**Design.** On compaction:

1. Split messages at the `keep_recent` boundary.
2. Call the LLM with the structured 9-section summary prompt
   (already used by the session-end path — ported from Claude Code).
3. Write the summary text into `JournalEntryKind::Compaction`
   with `{ summary_text, model_used, pre_tokens, post_tokens,
   timestamp }`.
4. Inject the summary into the compacted message list as a user
   message tagged `[PRIOR SESSION SUMMARY]`.
5. On LLM failure: fall back to the existing heuristic and
   annotate with `fallback_reason`. Never break the session.

Model choice: session's configured model by default. Optional
`[native_executor] compactor_model` in config for a cheaper
dedicated summarizer (coordinator = opus, compactor = haiku).

**Acceptance.** Live run driven past the soft-pressure threshold:
journal's `Compaction` entry contains a real summary (500-4000
chars), a subsequent turn demonstrates the agent remembers prior
work. Mocked-failure test: `fallback_reason` is set and heuristic
runs.

**Scope estimate.** ~200 LOC across `resume.rs`, `journal.rs`,
`config.rs`. One session.

---

## Step 3 — Tokenizer-aware token counts

**Goal.** Swap the `chars/4` heuristic in `ContextBudget` for a
real tokenizer so compaction pressure fires at the right time.

**Why.** The 4.0-chars-per-token constant systematically undercounts
for code (which runs closer to 3.0-3.3 chars/token). On a 32k
window, the soft-pressure threshold therefore fires late — by the
time we say "compact at next boundary," the next turn has already
blown the window. Self-dispatched work is disproportionately
affected because it runs longer.

**Design.** `tiktoken-rs` crate (bundles cl100k_base in-binary, no
network, no Python). Model → tokenizer map:

| Model family                               | Tokenizer    |
|--------------------------------------------|--------------|
| `gpt-4o`, `gpt-4.1`, `o1-`, `o3-`, `o4-`   | o200k_base   |
| everything else                            | cl100k_base  |

cl100k as the Anthropic/Qwen/Gemini approximation is ~5% off from
true — good enough for pressure detection. Perfect counts aren't
the point; replacing 25% systematic undercount with 5% jitter is.

Tokenizer loads are expensive (~10-50ms). Cache per-model in a
`OnceLock`. On load failure, fall back to `chars/4` with a single
warn-log — never panic, never break the session.

**Where.**

- `Cargo.toml` — `tiktoken-rs = "0.7"`.
- New `src/executor/native/tokenizer.rs`.
- `src/executor/native/resume.rs` — `ContextBudget` gets a
  `model: Option<String>` and uses the tokenizer when set.
- `src/executor/native/agent.rs` — `with_window_size(...)`
  call-site passes the model through via a new `.with_model(...)`
  builder.

**Acceptance.** Existing pressure-detection tests pass. New
regression test: for a fixed 5k Rust-source corpus, real count is
within expected range and the old heuristic undercounts by ≥15%.

**Scope estimate.** ~150 LOC. Could land alongside Step 2 if
it fits cleanly; otherwise separate.

---

## Step 4 — External-executor class

**Goal.** Treat claude / codex / amplifier / future peers as a
uniform class of "external subprocess agent loops," so adding a
new backend is config, not code.

**Why.** The existing `executor=claude` path is one-off — it has
its own `spawn_claude_process`, `stdout_reader`,
`agent_thread_main` branching, stream-json parser. If we add
codex support the same way, we duplicate all of it. The abstraction
we want: a trait (or config-driven command template) that says
"here's how to spawn this external executor, here's how to feed it
a prompt, here's how to read its output." Then claude, codex,
amplifier are all entries in the same config surface.

**Two-phase delivery.**

- **4a — Design doc.** Survey how claude, codex, amplifier each
  want to be invoked (stdin vs. args, stream-json vs. plain,
  session-resume mechanism, error surfacing). Propose the common
  abstraction. Write up for sign-off before any code.
- **4b — Implementation.** After sign-off on the design.

**Scope estimate.** Design: one session. Implementation:
probably 1-2 days depending on how many external executors get
plumbed in the first pass.

**Priority.** Last of the four because 1-3 move the native
executor forward (the self-bootstrap target), while Step 4 is
about making external executors prettier. Both matter; ordering
reflects the goal.

---

## What we are explicitly NOT doing

This is the deferred list, captured here so it persists across
sessions and future contributors see the reasoning:

- **Anthropic direct-API `cache_control` on outbound blocks.**
  Only matters for users who go through `AnthropicClient` with
  a direct ANTHROPIC_API_KEY. That's the minority path:
  - `executor=claude` → `claude --print` → Claude Code handles
    caching internally.
  - OpenRouter → `openai_client.rs` already sets `cache_control`
    on its passthrough.
  - OpenAI direct → server-side auto-cache.
  - SGLang / local → auto-cache.
  - Anthropic direct-API-key → currently uncached, and would
    benefit from `cache_control` — but the population of users
    on that path is small enough that the work is a "nice to
    have if anyone ever asks." Future item.
- **PyO3 Python bindings.** Dismissed. MCP solves the
  ecosystem-access need (spawn Python MCP servers for
  embeddings, retrievers, evaluators) without bringing a
  Python runtime into the binary.
- **Permissions model** (allow/deny per tool, globs, confirmation
  prompts). Important for shared / multi-user deployments. Defer
  until a concrete multi-user use case shows up.
- **OpenTelemetry** for multi-agent traces. The journal already
  gives per-session traceability; OTEL is for cross-agent /
  cross-run observability which isn't load-bearing yet.
- **Explicit eval mode** (`wg nex --eval-mode` for TB2 /
  SWE-bench). Worth doing to calibrate the small-model claim,
  but after the features are in — no point benchmarking before
  the improvements ship.
- **Streaming tool output verification.** Worth checking that long
  bash / web_fetch calls stream partial output rather than buffer
  — bug-class work, not roadmap.
- **Middleware / hook registry** (Open SWE-style pre/post-step
  hooks). Our `state_injection` covers 80% of the use case;
  resist abstracting until a second concrete need appears.

## Rollback

- Step 1 (MCP): the `McpManager` is optional. `--no-mcp` flag
  disables it. `[[mcp.servers]]` config defaults empty.
- Step 2 (LLM compaction): revert enriches `JournalEntryKind::Compaction`;
  old entries without the new fields still deserialize.
  `summarize_messages` heuristic stays in place as the fallback.
- Step 3 (tokenizer): behind a fallback — load failure drops to
  `chars/4`. Revert removes the dep and the module.
- Step 4: implementation rollback restores the existing
  per-executor code paths.
