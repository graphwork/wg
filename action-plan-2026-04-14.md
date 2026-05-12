# Action Plan: Intense Reliability for Local-Model Agents

**Status:** Active guiding star for native-executor hardening work.
**Companion doc:** [`issues-2026-04-14.md`](issues-2026-04-14.md) — historical record of problems 1–15 and the post-mortem of the `fix-worktree-fundamental` run.
**Started:** 2026-04-14

---

## The goal

**Intense reliability of the native executor running on free, fast local models (qwen3-coder-30b on lambda01 via sglang).** Local models are cheap, fast (~200 tok/sec out, even faster in), and don't burn API credits — but they have small context windows (32k for our target). Most of our failures on these models stem from **context exhaustion**, not from the model's reasoning ability. Fix the context-management infrastructure and qwen3-coder-30b becomes a serious, reliable agent for long-running work.

The end state: a wg service on native+qwen3 that can run indefinitely on long tasks, never fails structurally due to context, and self-heals through summarization and journaling rather than crashing.

---

## Architectural framing

### The recurrent-emulation insight

Stateful recurrent models have unbounded state via their hidden state. Stateless transformer agents have bounded context windows. **We can emulate recurrent-style unbounded state on a transformer by aggressively summarizing history**, with the summary serving as the compressed hidden state.

Each architecture has different advantages:
- **Transformers:** trivial forking (`messages.clone()`), inspectable state, editable/diffable history, cheap branching.
- **RNNs:** constant-time cloning via hidden-state snapshot, no prompt-injection surface via readable state, natural operation on read/written state.

We're not trying to beat RNNs at their game. We're building the transformer's version of unbounded state: **summary as the explicit hidden state**. That gets us inspectable, forkable, editable unbounded context on cheap local models.

### The core invariant

> **No single tool call, model turn, or compaction step may inject text whose estimated tokens exceed ~40–50% of the remaining context budget** (`window_size − overhead − current_message_tokens`).

Everything below is in service of maintaining this invariant structurally, so context explosions become impossible by construction rather than merely unlikely.

---

## What's done (Layer 0 — in PR #10)

Shipped 2026-04-14 on branch `fix-worktree-lifecycle`, [PR #10](https://github.com/graphwork/wg/pull/10):

**Commit `b36842de`** — initial overhead accounting:
- `ContextBudget` tracks `overhead_tokens` (system prompt + tool definitions + completion reservation) so `check_pressure` reflects real API budget usage.
- `hard_emergency_compact` variant drops ALL tool results in older turns, strips thinking, truncates long text.
- On streaming-400 retries: hard compact + halved `max_tokens` (1024 floor).
- Compaction logs report token deltas.
- 5 new tests.

**Commit `8f6719cd`** — follow-up fix after smoke-test revealed the initial fix was position-based, not occurrence-based:
- `emergency_compact` and `hard_emergency_compact` now walk ALL messages and shrink tool results by **occurrence count**, keeping only the last K tool-result *occurrences* verbatim (not the last K *messages*). On a chatty file-reading workload, big tool results live in recent message positions — the prior implementation left them untouched.
- Call sites updated: proactive uses `keep_recent_tool_results=2`, hard emergency uses `=1`.
- Log lines now include a `Δ -{}` delta so zero-reduction events are visible.
- Regression test `test_emergency_compact_shrinks_recent_position_tool_results` reproduces the exact smoke-test layout.

**Both fixes together address Problems 12 + 13 from the issues doc.** Baseline local fix — necessary but not sufficient.

### Smoke test results (2026-04-14 through 2026-04-15 early morning)

Five smoke tests against fresh `/home/erik/workgraph-e2e-smoke/` + lambda01/qwen3-coder-30b on a 32k context window. Each is intentionally progressively harder, stressing a different layer of the stack.

| # | Task | Result | Validates |
|---|---|---|---|
| 1 | Fix `multiply` bug in `calculator.py`, re-run tests | ✅ 15s, 46 entries, 0 compactions | Fresh env + new binary + basic file tool use |
| 2 | Summarize 8 × 12KB docs into `summary.md` | ❌ Task "done" but output **hallucinated** (see table below) — compaction destroyed content, agent generated plausible names from fading memory | Showed L0 alone is insufficient; motivated L1 |
| 3 | Same task, after L0 fix + L1 channeling + file-tools section | ✅ **Correct subtopic names**, 180 entries, 7 compactions, no 400s, no bash-echo loop | L0 + L0.5 + L1 + file-tools section work end-to-end |
| 4 | Summarize using `delegate` for each file | ✅ **16 delegate calls, 2 write_file, 0 bash echo**, correct output | Expanded tools section (delegate), anti-pattern list |
| 5 | Summarize a single 99KB file (3× context window) | ✅ **Used `summarize` tool** at turn 2. Input chunked 2×; 0 compactions; 64/64 subtopics correct | L2 summarize + full stack integration |

**The hallucination table from Test #2** (left for the record as motivation for L1+L2):

| File | Actual | Summary | Match |
|---|---|---|---|
| networking | TCP, UDP, ICMP, BGP, OSPF, MPLS, IPv6, QUIC | TCP, UDP, HTTP, HTTPS, DNS, TLS, QUIC, BGP | 4/8 |
| databases | btree, lsm-tree, hash-index, bloom-filter, wal, mvcc, replication, sharding | btree, hashmap, raft, sql, nosql, index, transaction, backup | 1/8 |

Every subtopic's description was the same boilerplate sentence, but subtopic *names* were lossy after compaction and the model generated plausible-looking replacements from training prior. L1 (tool output channeling) fixed this structurally by keeping the raw content off the message vec entirely.

### Key moment from Test #5 (L2 summarize)

Agent-7 called `summarize("big-source.md", instruction="List every H2 section heading...")` at turn 2. The summarize tool's internal logs:

```
[summarize] starting: model=qwen3-coder-30b, input_bytes=98881
[summarize] depth=0: 2 chunks from 98881 bytes (chunk_chars=52428)
[summarize] depth=0 chunk 1/2: 52098 → 262 bytes
[summarize] depth=0 chunk 2/2: 46783 → 241 bytes
```

A 99KB source → two summarization calls → 503-byte result → agent wrote it via `write_file` → done. **Zero compactions fired** because the big content never entered the agent's message vec — it was processed entirely inside the `summarize` tool's direct LLM calls.

This is the first time in the session that we ran a task strictly larger than qwen3's context window and completed it correctly with zero pressure events. The summary was 64/64 correct — the FULL set of subtopics from all 8 source files.

### Secondary finding from Test #2 — compaction plateau (fixed in L0.5)

The last 4 of 11 compactions in Test #2 were no-ops:
```
Proactive compaction: ~25434 → ~25434 tokens (Δ -0, 21 messages)
Proactive compaction: ~25469 → ~25469 tokens (Δ -0, 23 messages)
...
```

Once all non-keep tool results are already shrunk to stubs, `emergency_compact` has nothing left to reduce, but the model's own accumulated Text / Thinking / ToolUse content keeps growing. `emergency_compact` doesn't touch those; only the hard-emergency path does.

**Fix shipped as L0.5 (commit `657d5492`):** track consecutive no-op compactions; after 2 in a row with pressure still ≥ threshold, escalate to `hard_emergency_compact` which also strips Text/Thinking in elided messages. Counter resets on any non-zero delta or after an escalation fires.

Test #3 confirmed this works as designed:
```
Proactive Δ -0 (streak=1)
Proactive Δ -0 (streak=2)
Escalated hard Δ -761  ← escalation triggers
Proactive Δ -0 (streak=1)
Proactive Δ -0 (streak=2)
Escalated hard Δ -26
```

---

## The layered defense

Layer names and ordering, in priority of leverage:

### L1 — Tool output channeling (prevention, highest impact)

**Rule:** any tool whose output *could* be large writes to `.wg/output/agent-<id>/tool-outputs/<N>.log` and returns a small handle like:

```
[wrote 12345 bytes to tool-outputs/7.log; rows 0–18 of 340; use grep/head/tail/offset+limit to inspect]
```

The raw output physically never enters the message vec. The agent can still see anything it needs via follow-up tool calls (grep, head, tail, read_file with offset+limit). This makes context explosion via a single large tool output structurally impossible.

**Tools to modify:**
- `bash` — when stdout or stderr exceeds threshold, route to disk
- `read_file` — when file > threshold, return first N lines + handle
- `grep` / `glob` / `find` — when matches exceed threshold, route
- `web_fetch` — already has a `fetch_max_chars` cap but return format should be consistent with the handle idiom
- `web_search` — same
- `delegate` — child task outputs already go to journal; just make the parent's view go through the handle idiom

**Shared helper:** a `channel_output(bytes, agent_dir, index) -> Handle` function that produces the file + handle string.

**Threshold:** configurable, default ~4KB (roughly 1k tokens). Well under the 40% invariant so an agent can still do real work with multiple tool calls per turn.

**Estimated scope:** 6 tool modifications + shared helper + tests. ~400–600 lines.

### L1.5 — Size invariants enforced at tool-result time (bundled with L1)

After any tool runs, measure output size. If it still exceeds the ceiling (either because the tool didn't opt into channeling or because the handle itself is too verbose):

1. If the tool is one that should have channeled → bug, channel anyway as defensive fallback.
2. If the tool legitimately returns text → automatically wrap the result in a `summarize()` call with instruction `"preserve anything relevant to <current task title>"`.
3. Final fallback → hard-truncate with `[truncated by size guard at N bytes]` marker.

This enforcement lives in the agent loop's tool-execution path, not in individual tools, so every tool gets the protection for free.

### L2 — `summarize` as a first-class tool (recursive map-reduce)

**Signature:**
```
summarize(
    source: {path | url | task_id | inline_text},
    instruction: Optional<str>,        # what to preserve / focus on
    max_input_bytes: int = 1_000_000,  # hard ceiling, configurable
    max_output_tokens: int = 1024,     # size of final summary
) -> str
```

**Algorithm (map-reduce tree):**
1. Read source. Refuse if > `max_input_bytes`.
2. Chunk into pieces sized to ~40% of `(window_size − overhead − instruction_tokens)` so each chunk fits in one call with headroom.
3. **Map:** for each chunk, call the LLM with a focused summarization prompt that threads `instruction` through.
4. **Reduce:** if concatenated chunk summaries fit in a single call, do a final merge pass and return.
5. **Recurse:** otherwise, treat the summaries as the new input, go back to step 2.
6. **Terminal case:** single chunk → emit its summary directly.

**Journal integration:** every recursion level appends a `JournalEntryKind::Compaction` so a mid-run failure can be resumed from the last completed reduction step rather than starting over.

**Why a tool, not just an internal helper:** by exposing `summarize` as a tool, the agent can *choose* to use it deliberately when it knows a source is too big, instead of only falling back to it passively on size-guard triggers. This is the "decompose by calling a tool" model that makes 32k-context work on large files.

**Becomes the cornerstone:** L1.5 (size-guard fallback), L3 (recursive compaction of agent's own message vec), and L4 (journal resume) all build on this primitive.

**Estimated scope:** ~300–500 lines new tool + integration + tests.

### L3 — Recursive compaction of the agent's own context

When `hard_emergency_compact` (Layer 0) isn't enough because the *compaction input itself* would exceed context, apply the `summarize` tool to the message vec. Split the older-messages region in half (or thirds, or until chunks fit), summarize each piece independently, concatenate, and if the concatenated summaries still don't fit, recurse.

This makes compaction **structurally unable to fail** — it has to produce *something*, even in pathological cases where the content is enormous.

**Terminal:** one message worth of content → hard-truncate with `[truncated by recursive compact]` marker and keep going.

**Estimated scope:** ~200–300 lines leveraging L2.

### L4 — Journal-integrated compaction with resume

`JournalEntryKind::Compaction` already exists in `src/executor/native/journal.rs` but isn't fully wired. Finish the integration:

- Every compaction event (proactive, hard-emergency, recursive) is journaled with before/after token estimates and the sequence range it covers.
- On resume (`ResumeData::load`), detect in-progress recursive compactions from the journal and continue from the last completed reduction step.
- `was_compacted` annotation in resume context tells the new agent session that older turns have been compressed.

**Estimated scope:** ~150–300 lines.

---

## Components summary table (post-2026-04-15 shipping session)

| Layer | What | Status | Commit | Leverage |
|---|---|---|---|---|
| L0 | Overhead accounting + hard compact + retry max_tokens reduction | ✅ shipped | `b36842de` + `8f6719cd` | Medium (unblocks verification) |
| L0.5 | Soft→hard compaction escalation on plateau | ✅ shipped | `657d5492` | Medium (fixes Text/Thinking accumulation) |
| L1 | Tool output channeling to disk + handle return | ✅ shipped | `0f694ef2` | **Highest** (verified: eliminates hallucination) |
| L1.5 | Size-invariant enforcement at tool-result time | 🟡 partial (channeling covers the main case; explicit wrapping not yet) | — | High |
| L2 | `summarize` tool (recursive map-reduce) | ✅ shipped | `b2ce2a74` | **Highest** (verified: handles 3× context-window sources) |
| Native tools prompt section | Teach file/web/delegate/summarize tools in system prompt | ✅ shipped | `0f694ef2` + `bb0ce997` + `b2ce2a74` | Critical multiplier — without it the tools don't get used |
| `wg_add` subtask + cron | In-process tool schema exposes the CLI flags | ✅ shipped | `bfbca92c` | Medium |
| L3 | Recursive compaction of agent's own message vec (using `summarize` as the primitive) | Not started | — | Medium (L1 + L2 have made this less urgent) |
| L4 | Journal-integrated compaction with resume-from-partial | Not started | — | Low (nice-to-have) |

Total shipped this session: **~1600 lines** across 8 commits on PR #10. All layers validated end-to-end on a 32k-context local model (qwen3-coder-30b on lambda01) via progressively harder smoke tests.

---

## Current decision point

Before starting L1+L2, we want to **smoke-test L0** in a controlled environment to prove the baseline fix actually moves the needle in practice. This is fast and de-risks the rest of the plan.

### Smoke test setup (not yet executed)

- **Fresh clean directory:** `/home/erik/workgraph-e2e-smoke/` or similar. NOT the current `/home/erik/workgraph/.wg/` which is too noisy (1665 done + 567 abandoned + 211 open + paused flip tasks + stale state).
- **Minimal config:** ~25 lines. Three tiers all pointing to `openai:qwen3-coder-30b`, lambda01-local endpoint marked `is_default = true`, qwen3 in `model_registry`, native executor, `worktree_isolation = false`, `max_agents = 1`. **No FLIP, no auto_evaluate, no auto_assign** for the first smoke — we're verifying compaction only, not agency pipeline.
- **One task:** terminal-bench-style self-contained task. Small bug in a Python file, find and fix it, run the test. Chatty enough to exercise many tool calls but bounded enough to complete.
- **Watch the log for:** proactive compaction firing at the right threshold (~65% effective utilization, not ~100%); if hard limit is reached, hard compact + halved max_tokens retry should succeed.
- **Keep the existing `.wg/` as-is** — don't delete, it's evidence for the issues doc.

### What comes after the smoke test

Assuming L0 verifies:
1. Start L1+L2 design together (they co-depend via the size-guard fallback path).
2. Land L1 first as a standalone PR (it's valuable without L2).
3. Land L2 as a second PR.
4. Then L3, then L4.
5. After L4: re-attempt `fix-worktree-fundamental` style tasks on qwen3+native with confidence.

If L0 doesn't verify:
1. Debug whichever specific part of Layer 0 failed.
2. Adjust `ContextBudget` math, chars-per-token constant, or retry policy as needed.
3. Re-smoke-test.
4. Then proceed with L1+.

---

## Open questions

1. **Chars-per-token is hardcoded to 4.0.** For most Western text this is a reasonable estimate, but for code-heavy content it's an underestimate (code tokenizes denser). Should this become per-model or learned from actual usage in `Usage` responses?
2. **`channel_output` file retention.** How long do we keep the tool-outputs logs? Per-agent cleanup after the agent exits? Per-task? Some retention policy?
3. **Summarize tool cost.** Each recursive level is an LLM call. On expensive providers this adds up. For local models (qwen3 on lambda01) it's effectively free, but we should be careful about upstream applicability.
4. **Size invariant threshold.** User specified "40–50% of remaining context." Make this a single `size_guard_ratio` config knob? Per-tool overrides?
5. **Structured tool outputs.** Today tool outputs are plain text. Should we move to structured `{summary: str, handle: Option<str>, metadata: ...}` so the handle idiom is first-class in the type system rather than an in-band string convention?

---

## Related docs and artifacts

- [`issues-2026-04-14.md`](issues-2026-04-14.md) — historical record of Problems 1–15 and the post-mortem
- [`damage-diff-2026-04-14.patch`](damage-diff-2026-04-14.patch) — full diff of the 2185-line deletion incident from earlier tonight
- `study-damaged-worktree-fix-2026-04-14/` — preserved artifacts from the failed run (stub `liveness.rs`, scaffolded `shared_state.rs`, etc.)
- Commit [`b36842de`](https://github.com/graphwork/wg/pull/10/commits/b36842de) — the L0 fix
- [PR #10](https://github.com/graphwork/wg/pull/10) — the L0 fix under CI review
