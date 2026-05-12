# Unified Path Forward

**Status:** plan of action. Intended to be executed in order.
**Date:** 2026-04-16
**Context:** end of the session where we hardened 13 commits worth of infrastructure and discovered that the tool-fragmentation problem was blocking real usage. This doc captures the unified shape the system should converge toward and the sequence to get there.

## The thesis

We've built two entangled systems of tools:

1. **Reflex tools** the model reaches for without thinking: `read_file`, `bash`, `write_file`, `web_fetch`, `grep`. These have wide training coverage; the model uses them by pattern-match.
2. **Deliberate tools** we've added for specific patterns: `summarize`, `survey_file`, `research`, `deep_research`. These require the model to *decide* to use them instead of the reflex tools.

The deliberate-vs-reflex split **doesn't work with small models.** Observed twice this session:

- `write_file` fires on "write me a haiku" because the word "write" is in the prompt.
- `survey_file` was ignored in the smoke test — the model ran 15 bash+read_file greps instead, produced a fabricated answer, and never invoked the survey machinery at all.

The model's tool-selection pathways are dominated by lexical cue and training-set frequency. Descriptions help frontier models; small models are barely steered by them. **The fix isn't better tool descriptions — it's putting the intelligence inside the reflex tools.**

## The unification

### `read_file(path, query?)` absorbs `summarize` and `survey_file`

```text
read_file(path)                        # classic: return lines
read_file(path, offset=N, limit=M)     # classic: return slice
read_file(path, query="...")           # NEW: answer the query
```

When `query` is present:

- If file fits in one LLM call → single call, return the answer text
- If file doesn't fit → run the cursor-based traversal (the internals of the current `survey.rs`) with the query, return the final answer

Caller sees a text answer. Doesn't need to choose which tool to use. Doesn't need to know whether map-reduce or traversal is happening internally.

### `web_fetch(url, query?)` absorbs `research` (single-page version)

```text
web_fetch(url)                         # classic: fetch + extract
web_fetch(url, query="...")            # NEW: fetch + answer the query
```

Same pattern. The current `research` tool's search+fetch+summarize pipeline becomes:

- `web_search` for finding URLs (unchanged)
- `web_fetch(url, query)` for answering from each URL

`deep_research` stays as a top-level tool because its decompose→fan-out→synthesize shape is genuinely different (multi-LLM orchestration, not a single call). But internally it uses `web_fetch(url, query)` rather than its own bespoke pipeline.

### Ancillary capabilities become parameters

Instead of new tools for background execution, interruptibility, and forking, these become **parameters on the unified tools:**

| Capability | Shape |
|---|---|
| Background execution | `read_file(path, query, background=true)` returns a handle; caller polls or awaits |
| Interruptibility | Inner loops check a shared cancel flag between turns; outer session has `/stop` slash command |
| Fork | `read_file(path, query, fork_from=<journal_path>:<turn>)` — resume inner loop from a journal |
| Join | Semantic merge via deep_research-style synth step, not a primitive |

The model doesn't need to know any of this. The outer session's UI exposes `/background`, `/stop`, `/fork` as slash commands that drive the underlying parameter machinery.

### What disappears or consolidates

- `summarize` (tool) → deleted. `read_file(path, query="summarize focusing on X")` is the replacement. Internal map-reduce machinery (`recursive_summarize`) stays as a library function — the query-mode dispatcher picks it when the file is large and the query is generic.
- `survey_file` (tool) → deleted. Its internals (cursor loop, `read_chunk`/`note`/`finish` state) become the backend for `read_file` query-mode on large files, named `file_query.rs` internally.
- `research` (tool) → deleted. `web_fetch(url, query)` handles single-page; `deep_research` handles multi-source.
- `deep_research` (tool) → stays. Multi-LLM orchestration is a genuinely different capability.

### Tool count before → after

Before: `read_file`, `write_file`, `edit_file`, `grep`, `glob`, `bash`, `wg`, `wg_done`, `wg_add`, `wg_fail`, `wg_log`, `wg_artifact`, `bg`, `web_search`, `web_fetch`, `summarize`, `research`, `survey_file`, `deep_research`, `delegate` = **20 tools**

After: `read_file`, `write_file`, `edit_file`, `grep`, `glob`, `bash`, `wg`, `wg_done`, `wg_add`, `wg_fail`, `wg_log`, `wg_artifact`, `bg`, `web_search`, `web_fetch`, `deep_research`, `delegate` = **17 tools** (fewer, and the three that dropped were the ones the model ignored anyway)

## Readiness gates before "let loose"

"Let loose" = turn on `wg service start` with native executor dispatching real work autonomously. Today we don't do this because we've repeatedly watched native-exec agents hallucinate destructively. Each gate below closes one failure mode.

### Gate 1: Tool unification landed and live-tested

- `read_file(path, query)` implemented, query-mode uses `file_query.rs` backend
- `web_fetch(url, query)` implemented
- `survey_file`, `research`, `summarize` removed as top-level tools
- Live test: `wg nex` with a question about a large file produces a coherent, non-fabricated answer
- Pass criteria: qwen3-coder invokes `read_file(..., query=...)` on its own without explicit prompting, and the inner traversal loop keeps context bounded

### Gate 2: Task-type tool scoping (per `tool-scoping-for-agents-research.md`)

The revised table from that doc:

| Task type | Tools |
|---|---|
| `.flip-*` | nothing — pure in-context reasoning |
| `.assign-*`, `.place-*` | `wg` read-only subset |
| `.compact-*` | read + write `.wg/context.md` only |
| `.evaluate-*` | full agent (escalation/repair path) |
| Regular tasks | full, cwd-sandboxed writes |

- `ToolRegistry::for_task_type(task_id, exec_mode)` constructor
- `spawn/execution.rs` picks task-type from task-id prefix, passes to registry builder
- Meta tasks can't accidentally write to the main tree even if they hallucinate absolute paths
- Evaluate agents CAN mutate graph state (wg_fail, wg_add) to drive auto-repair

### Gate 3: Live dispatched test, simple task

With coordinator running native executor, dispatch one well-scoped real task (e.g. "add a docstring to function X in file Y"). Observe:

- Agent spawns in worktree (gating works)
- Write goes to worktree, not main tree (sandbox works)
- Task completes, squash-merges to main (wrapper works)
- Worktree preserved for inspection (sacredness works)
- No ghost agents spawn after stop (ghost-spawn fix works)
- Chrome orphan reaper fires on next session if needed

### Gate 4: Live dispatched test, non-trivial task

One task that exercises `read_file(path, query)` on a large file, produces a non-fabricated answer, completes within turn budget. Validates that the inner-loop autocompaction holds up under real agent usage.

### After Gate 4: Let loose

Turn on `auto_assign`, `auto_place`, possibly `auto_create`. Let the coordinator dispatch the SearXNG follow-up, the deep_research live test, whatever real work is queued. Watch the first hour carefully; degrade gracefully if anything smells wrong.

## Order of operations

Ordered by dependency + risk.

1. **Unify `read_file`** (Gate 1, part 1)
   - Move `src/executor/native/tools/survey.rs` → `src/executor/native/tools/file_query.rs` internally
   - Add `query` parameter to `read_file` in `src/executor/native/tools/file.rs`
   - When `query` present → delegate to `file_query::run_query_on_file(path, query, provider, max_turns)`
   - Remove `survey_file` from tool registration
   - Keep `recursive_summarize` as internal library for the "file too large + generic summary" case
   - Build, test, install, live-test with a real question

2. **Unify `web_fetch`** (Gate 1, part 2)
   - Symmetric shape. `web_fetch(url, query?)` delegates to a `web_query` backend when query is present
   - Reuses `read_file`'s query-mode infrastructure since fetched content is equivalent to a file
   - Remove `research` as a top-level tool; rewrite `deep_research` to use `web_fetch(url, query)` internally for per-page

3. **Task-type tool scoping** (Gate 2)
   - `ToolRegistry::for_task_type()` constructor
   - Per-task-type allowlists per the revised table
   - `spawn/execution.rs` plumbing
   - Unit tests for each category

4. **Dispatched smoke test** (Gates 3 + 4)
   - Coordinate with a fresh `.wg/` in a scratch dir
   - Dispatch a known-good simple task
   - Watch every phase
   - Then a non-trivial task
   - Document failure modes if any

5. **Let loose in wg itself** (real work)

## Deferred / separate design

These are named so they don't get lost, but they're not blocking Gate 4:

- **bwrap sandboxing** — kernel-enforced read-only binds on the source tree for meta tasks. Linux-only. User is writing the design doc separately.
- **Fork/join primitives** — `wg nex --fork-from <journal>:<turn>` and `wg nex --join ...`. Useful for exploration ("try 3 approaches, keep best"). Shape depends on what we learn from Gates 1-4.
- **Agent UI backgroundability** — detach/reattach nex sessions, watch via `wg agents`, slash commands for `/background`, `/stop`, `/fork`. Touches the nex command more than the tool shape.
- **r-indexed Common Crawl** — separate infrastructure project.

## What "polish" means operationally

Listing concretely so the phrase isn't fuzzy:

- ✅ Worktrees never destroyed except by explicit user action
- ✅ Worktree creation gated so meta tasks don't burn disk
- ✅ `write_file` can't escape cwd even on hallucinated absolute paths
- ✅ `wg kill` tree-kills (zombies solved)
- ✅ Ghost-agent race after `wg service stop` fixed
- ✅ Chrome orphans get reaped before they block new launches
- ✅ Disk cache is human-readable files (grep-able, distributed-safe)
- ✅ Bland system prompts (no "expert software engineer" priming for every haiku)
- ✅ `openai` renamed to `oai-compat` throughout (internal + user-facing)
- ✅ Google News opaque URL resolution in-band
- ✅ SearXNG backend wired and docker-running
- ✅ deep_research tool landed (not yet live-tested)
- ✅ survey_file prototype landed (not yet live-tested — Gate 1 folds it in)
- ⚠️ Tool unification not yet done
- ⚠️ Task-type tool scoping not yet done
- ⚠️ End-to-end dispatched agent smoke test not yet done
- ⚠️ Native executor not yet permitted for self-dispatched work (per memory)

The three ⚠️s are the Gates. They're the remaining polish.

## How to use this doc

- **Next session starts here.** `cat docs/design/unified-path-forward.md` to reorient.
- Order of operations is the agenda.
- Gates are exit criteria; don't move past a gate without passing it.
- "Deferred" section is the "don't forget" list for later; nothing there blocks anything in the main sequence.
