# Portable Pi-quality log rendering

**Task:** `design-portable-pi`  
**Status:** design/research only; no production code changed  
**Observed runtime:** WG checkout on 2026-07-26; installed `@earendil-works/pi-coding-agent` **0.82.0**

## Decision

Adopt a **hybrid A+B design**:

1. **Always** retain immutable executor-native bytes and derive a richer, provider-neutral semantic transcript. WG renders that transcript itself, so every historical run remains readable without Pi.
2. For a Pi run, **optionally and only when Pi is available**, retain/copy its native session and invoke Pi's supported credential-free HTML export during capture. Store the standalone HTML as a disposable, versioned high-fidelity cache/attachment. On a cache miss, a view may request one background re-render; it must immediately fall back to WG's neutral renderer.
3. Do **not** scrape/replay Pi's terminal, ANSI stream, interactive components, or generated HTML into WG's TUI. Pi 0.82 exposes supported events, sessions, SDK subscriptions, TUI *component construction*, and HTML export, but no supported general transcript-to-RenderTree/Markdown API. Exact Pi TUI reuse would therefore couple WG to terminal state and internal composition.
4. Ask upstream for a supported headless `renderTranscript(entries, options) -> RenderTree | HTML` API. If one appears, it can replace only the optional cache producer; the persisted native and canonical layers do not change.

This gives Pi users an exact Pi-owned standalone transcript when possible and a substantially better WG TUI everywhere. Direct Codex/Claude never import, probe, spawn, or otherwise depend on Pi.

## 1. Evidence and reproduction

### 1.1 Real Pi run

I analyzed retained run `agent-690` (`make-pi-the-2`, metadata executor `pi`, model `openai-codex:gpt-5.6-sol`) rather than a hand-written happy-path record:

| retained evidence | observed |
|---|---:|
| `raw_stream.jsonl` | 30,896,955 bytes |
| turns (`turn_end`) | 43 |
| distinct `toolCallId`s | 58 |
| tool updates | 136 |
| tool ends marked error | 4 |
| assistant blocks | 40 thinking, 58 tool calls, 1 final text |
| turn usage records | 43 |
| summed Pi-reported cost | $2.871119 |
| canonical `stream.jsonl` | 22,376 bytes |

The completed canonical stream contains 1 init, 58 `tool_start`, 58 `tool_end`, 40 `thinking_chunk`, 43 `turn`, 1 `text_chunk`, and 1 result. Its accounting total is correct, but its presentation payload has lost:

- every `toolCallId`, tool argument, result content/detail, and all 136 updates;
- error messages (only `is_error` remains);
- message lifecycle and turn start/end boundaries;
- native event timestamps and actual durations (`duration_ms` is always zero);
- stop reasons, compaction/retry/extension errors, images, and nested-tool usage/detail.

The reduction is intentional in the current translator: the canonical enum has tool start/end names but no correlation IDs or payloads (`src/stream_event.rs:16-61`); the Pi bridge maps start/end at `src/stream_event.rs:492-515`, assigns translation-time `now_ms()` and zero duration, handles finalized content only under `turn_end` at `:518-604`, and drops every other Pi event at `:607`. Usage is correctly deduplicated at `turn_end` (`:455-457`, `:584-597`).

This is not a capture failure. Pi stdout is tee'd byte-for-byte into `raw_stream.jsonl` (`src/commands/spawn/execution.rs:2712-2729`). The post-exit bridge explicitly rewrites canonical `stream.jsonl` from those bytes (`src/commands/pi_stream_bridge.rs:28-52`).

### 1.2 Where the visible degradation occurs

There are actually **two lossy translations**:

1. **Post-exit canonical bridge.** It is adequate for liveness/accounting but not a transcript, as above.
2. **Live TUI parser.** Events/HighLevel/Pretty do not read canonical `stream.jsonl`; they independently parse bounded windows of native `raw_stream.jsonl` (`src/tui/viz_viewer/state.rs:18031-18175`, selected by `src/tui/viz_viewer/render.rs:6503-6734`). The Pi arm explicitly discards session/agent/message lifecycle, `turn_start`, every `message_update`, and `tool_execution_update` (`src/tui/viz_viewer/state.rs:7394-7410`). It keeps start args, but a tool result retains neither `toolCallId` nor tool name and extracts only the first text block (`:7412-7487`). `turn_end` keeps text/thinking but drops usage, stop reason and tool calls (`:7489-7558`). Unknown events disappear.

That destroys correlation in the most important real case: Pi documents that parallel starts are emitted in assistant source order, updates may interleave, and ends arrive in completion order (`installed docs/extensions.md:624-635`). WG's Events view can fold a result into a call only when the next parsed row happens to be a `ToolResult` (`src/tui/viz_viewer/log_render.rs:109-137`). Multiple starts followed by completion-order ends therefore appear as unpaired calls plus anonymous results. This is the concrete Pi overview failure.

`session-summary.md` cannot repair it. The bridge stores only the last assistant text, capped at 4,000 characters (`src/commands/pi_stream_bridge.rs:26,55-70`), and `wg show` reports only `Session summary: present (N words)`, not a transcript (`src/commands/show.rs:320-330,841-848`). True Raw remains forensic and byte-faithful, but JSON snapshots are not a legible overview.

### 1.3 Direct Codex control

For a direct Codex CLI control I inspected retained `agent-552` (`remove-workgraph-branding`, executor `codex`, model `gpt-5.6-sol`): 2,097,182 bytes containing 40 command start/completed pairs, 4 file-change pairs, 12 finalized assistant messages and one `turn.completed`.

WG ignores Codex started/updated bookkeeping, but each `item.completed.command_execution` already co-locates command, accumulated output, exit code and status. The parser therefore produces one correlated row per completed command (`src/tui/viz_viewer/state.rs:7267-7393`) instead of Pi's split, completion-order start/end problem. This localizes the material disparity to provider-shape translation/correlation, not Ratatui alone.

The Codex control is not perfect: current WG drops `file_change` and `turn.completed` usage. The proposed neutral schema fixes both providers. Crucially, direct Codex remains its own adapter and never needs Pi.

### 1.4 Pi's own supported export control

The Pi session referred to by the real run was still available as a 491,564-byte v3 session. The supported command

```text
pi --offline --export <session.jsonl> /tmp/design-portable-pi-session.html
```

completed without credentials or network and produced a 923,346-byte standalone HTML transcript. Pi documents `/export`, CLI `--export`, and RPC `export_html` (`installed README.md:544`; `docs/rpc.md:574-595`). This proves a supported Pi-owned high-fidelity cache is possible **if the Pi session is retained**, without replaying a terminal.

## 2. Supported Pi surfaces versus internals

The installed documentation and linked examples were read completely for JSON, RPC, sessions/session format, SDK, extensions, TUI and themes.

### Supported and suitable

- **JSON event stream:** explicitly for integrations/custom UIs (`docs/json.md:7`), with lifecycle, message deltas, correlated tool start/update/end (`:14-47`) and a versioned session header (`:64-78`).
- **RPC:** strict LF JSONL (`docs/rpc.md:30-39`), correlated tool events whose update is an accumulated replacement snapshot (`:971-1014`), stats (`:531-570`), durable entry cursors (`:694-730`), and `export_html` (`:574-595`).
- **SDK:** `AgentSession.subscribe()` is public (`docs/sdk.md:27-35,79-81,267-313`), and documented run-mode wrappers include Interactive/Print/RPC (`:998-1110`).
- **Session v3:** content blocks, tool-result `isError`, optional nested LLM usage, branches and compaction are documented (`docs/session-format.md:19-27,43-113`). Pi promises migration of old versions when loaded (`:19-27`).
- **HTML export command/RPC:** supported, standalone, and usable offline. Theme export colors are documented (`docs/themes.md:240-251`).
- **Extension tool/message rendering:** `renderCall`/`renderResult`, `registerMessageRenderer`, and `registerEntryRenderer` are supported for Pi's interactive UI (`docs/extensions.md:2179-2306,2793-2826`; examples `built-in-tool-renderer.ts`, `message-renderer.ts`, `entry-renderer.ts`).

### Supported, but not a neutral transcript renderer

The main package publicly exports `AssistantMessageComponent`, `ToolExecutionComponent`, and theme helpers (`dist/index.d.ts:28-29`). TUI components return width-dependent `string[]` lines and may contain ANSI (`docs/tui.md:9-27`); `ToolExecutionComponent` needs a TUI instance, cwd, live definition/state, expanded mode and image capabilities. These are supported building blocks for a Pi TUI, not a headless semantic rendering contract.

### Internal-only / unsuitable

- The HTML implementation declares `exportSessionToHtml` and `exportFromFile` in `dist/core/export-html/index.d.ts:31-36`, but `package.json:14-21` exports only `.` and `./rpc-entry`; consumers should use the supported CLI/RPC command, not import that internal path.
- Custom-tool HTML export itself invokes TUI renderers and converts ANSI to HTML (`dist/core/export-html/tool-renderer.js:4-7,63-103`). That is an implementation detail and an explicit warning against treating ANSI as a stable semantic interface.
- No docs or main-package export defines a generic transcript renderer, RenderTree, Markdown exporter, or event-to-component reducer.

**Conclusion:** Pi exposes supported structured **data** and supported standalone **HTML export**, but no supported structured Pi-visualization API. Exact TUI visualization is internal composition.

## 3. Options

### A. Improve WG's provider-neutral translator — **required baseline**

Create one executor-adapter boundary that produces a lossless-enough semantic transcript. Both canonical files and all WG views consume it; remove the second ad hoc TUI parser over time.

Pros: portable, deterministic, streamable, testable, executor-independent.  
Cons: WG must own presentation and tool summaries; it will be Pi-like rather than byte-identical to Pi.

### B. Optional Pi-installed renderer/cache — **use only supported HTML today**

At Pi-run completion, copy the native Pi session into the attempt directory and run supported `pi --offline --export` to a temporary file, then atomically install the HTML cache. A background view-time retry is allowed on cache miss/staleness.

Do **not** parse that HTML back into the TUI. WG's neutral RenderTree remains the in-app view. The HTML is a high-fidelity “Open Pi transcript” attachment. If upstream later exposes structured rendering, add a cache producer that emits RenderTree/Markdown and prefer it without changing storage.

Pros: exact Pi-owned presentation, credential-free, standalone historical file.  
Cons: completion-time Pi subprocess; requires a retained Pi session; HTML is not a Ratatui model; Pi export may migrate old sessions, so raw input must never be overwritten.

### C. Scrape/replay terminal output — **reject**

`PI_TUI_WRITE_LOG` is documented only as debug logging (`docs/tui.md:470-476`). Terminal replay depends on width, theme, focus, animation, cursor/OSC/image capabilities and interactive state. ANSI conversion is demonstrably an internal HTML-export implementation detail. It is fragile, inaccessible, hard to diff, unsafe to replay without strict control stripping, and cannot recover semantics after theme/width changes.

## 4. Storage contract

Per attempt:

```text
agents/<attempt>/
  raw_stream.jsonl                 # immutable native event bytes (existing)
  native/
    pi-session.v3.jsonl            # immutable snapshot if available
    manifest.json                  # hashes, executor, Pi/session versions
  semantic/
    transcript.v1.jsonl            # portable semantic events
    manifest.json                  # schema + native hash + translator provenance
  render/
    neutral-v1/<semantic-hash>/
      tree.json                    # disposable RenderTree cache
      transcript.md                # portable fallback cache
      manifest.json
    pi-html/<pi-version>/<session-hash>/
      transcript.html              # disposable but standalone Pi-owned cache
      manifest.json
  stream.jsonl                     # existing operational telemetry, unchanged initially
  session-summary.md               # existing resume aid, not transcript authority
```

Rules:

- Native files are append-only while live and immutable after finalization. Record byte length and SHA-256/BLAKE3; never migrate them in place.
- `semantic/transcript.v1.jsonl` is appendable and portable. Unknown native events become `unknown`/`notice` records with a source reference, never silent drops.
- Rendered products are caches. Delete/rebuild them freely from native+semantic inputs. Markdown is committed at capture so history remains pleasant even if all renderer software later disappears.
- Cache paths/manifest keys include input hash, renderer ID/version, semantic schema, Pi package version, WG/plugin compatibility (when applicable), theme, width class, expanded policy, and redaction policy.
- Direct `codex`/`claude` metadata selects only their adapters and `neutral-v1`; the `pi-html` directory and Pi feature probe are unreachable on those paths.

## 5. Neutral RenderTree v1

A deliberately small schema (JSON shown; JSON Schema should use a tagged union):

```json
{
  "schema": "wg.render-tree/v1",
  "source": {"executor":"pi","semanticHash":"b3:..."},
  "blocks": [
    {
      "kind":"turn", "id":"turn:7", "index":7, "status":"error",
      "children":[
        {"kind":"reasoning", "id":"msg:9:think:0", "markdown":"Check both branches.", "collapsed":true},
        {
          "kind":"tool", "id":"call:a", "name":"bash", "status":"ok",
          "summary":"$ cargo test", "input":{"command":"cargo test"},
          "updates":[{"text":"running 12 tests"}],
          "result":[{"type":"text","text":"12 passed"}],
          "usage":null, "parentId":null
        },
        {
          "kind":"tool", "id":"call:b", "name":"subagent", "status":"error",
          "summary":"subagent → review", "input":{"task":"review"},
          "updates":[], "result":[{"type":"text","text":"review failed"}],
          "error":{"code":null,"message":"review failed"},
          "usage":{"input":80,"output":12,"costUsd":0.003}, "parentId":null
        },
        {"kind":"assistant", "id":"msg:10", "content":[{"type":"markdown","text":"I found the failing branch."}]}
      ],
      "usage":{"input":200,"output":40,"cacheRead":100,"cacheWrite":0,"costUsd":0.02}
    }
  ]
}
```

Required block kinds: `turn`, `assistant`, `reasoning`, `tool`, `user`, `error`, `notice`, `usage`, `unknown`. Every block has stable `id`, ordered source references (native byte/record index), optional native timestamp, and extensible `meta`.

Tool rules:

- correlate by `toolCallId`, never adjacency;
- updates replace accumulated content when Pi says they are accumulated (`docs/rpc.md:1014`), while adapters may mark true deltas explicitly;
- preserve all text/image content blocks, opaque `details`, error flag/message, and nested `usage`;
- represent parallel starts in source order and completion status on the matching node;
- include `parentId` only when the executor supplies one. Pi's documented tool events have no parent-call field, so nested execution must not be invented. Nested LLM usage belongs on the tool node (`docs/session-format.md:93-101`).

A Markdown renderer maps the same tree to headings, collapsible-detail hints, fenced commands/results, error callouts, per-turn usage, and explicit turn separators. Ratatui maps it to styled spans. Neither reads provider raw JSON.

## 6. Feature detection, compatibility and fallback

### Detection

Only for `metadata.executor == "pi"`:

1. Locate the exact Pi used for capture if recorded; otherwise PATH.
2. Run `pi --version` with a short timeout and record stdout. Do not infer feature support from installation alone.
3. Prefer a capability probe against a tiny credential-free v3 fixture: `pi --offline --export fixture.jsonl temp.html`. Validate exit 0, nonempty regular output, expected HTML marker, and bounded size. No auth/provider call.
4. If a future structured renderer exists, require an explicit protocol handshake returning `{rendererProtocol, piVersion, inputSessionVersions, outputSchema}` before sending logs.

### Versions and provenance

Example cache manifest:

```json
{
  "cacheSchema":"wg.render-cache/v1",
  "renderer":{"id":"pi-cli-html","version":"0.82.0","protocol":null},
  "inputs":{"sessionVersion":3,"sessionHash":"b3:...","semanticSchema":"wg.semantic/v1"},
  "wg":{"version":"...","piPluginCompat":"0.2.0"},
  "options":{"theme":"dark","widthClass":"standalone","expanded":"user-toggle"},
  "createdAt":"...","status":"complete"
}
```

Pi session version and Pi renderer version are separate. WG's existing plugin handshake is `WG_PI_PLUGIN_COMPAT_VERSION = 0.2.0` (`src/pi_plugin/mod.rs:25-45`) and already protects WG↔plugin tools; record it for provenance, but do not pretend it guarantees Pi TUI rendering compatibility. A future structured renderer needs its own output protocol version.

### Failure behavior

Any of missing Pi, unsupported session version, failed probe, timeout, crash, malformed/oversized output, plugin mismatch, or renderer-schema mismatch:

- leave native and semantic files untouched;
- remove only the temporary cache;
- show neutral RenderTree/Markdown immediately;
- surface one nonfatal provenance note (“Pi enhancement unavailable: …”);
- never block task completion or make the log blank.

Unknown semantic fields/events render as a compact notice plus expandable JSON, ensuring forward schema drift fails visible rather than lossy.

### Re-rendering

- Capture-time render writes via temp+fsync+rename and gives the historical transcript a warm cache.
- On view, compare manifest/input hash/version/options. Use a valid old cache immediately. If stale and Pi is available, queue one background rebuild; never block first paint.
- An explicit `wg logs rerender <task> [--renderer neutral|pi-html]` should rebuild caches. `--all` must be opt-in and bounded.
- Upgrades create a sibling cache; do not delete the last known-good cache until the new output validates.
- Pi may migrate a copied session in memory/on its own working copy, never the immutable native snapshot.

## 7. Capture versus view and dependency budget

**Both, capture-first.**

- **During capture:** stream native bytes and semantic events continuously. On clean or failed termination, finalize hashes, Markdown/tree, copy the Pi session, and attempt Pi HTML once.
- **During view:** read neutral cache synchronously. Re-render only on a version/hash/options miss, in the background.

Additional Pi dependency:

- Pi executor runs: one extra **credential-free, offline subprocess per completed attempt** for HTML export; zero model calls, zero tokens/cost, no network, no auth. A view adds at most one subprocess per distinct cache key.
- Direct Codex/Claude/native runs: **zero** Pi processes, imports, probes, files, or package dependencies.
- Persisted readability: **zero** Pi dependency; Markdown/RenderTree and canonical events are sufficient.

## 8. Incremental implementation and tests

### Phase 0 — fixtures and characterization

- Add a sanitized, credential-free paired fixture set encoding the same flow in Pi JSON and direct Codex JSON: assistant deltas/final text, reasoning, two parallel tools, accumulated updates, success+error, multiple content blocks, nested tool usage, turn boundaries, retry/compaction/extension error, and usage/cost.
- Preserve separate real-shape excerpts from `agent-690` and direct Codex `agent-552`, scrub paths/prompts/secrets, and document extraction hashes.
- Characterization assertions must demonstrate today's Pi correlation/update/boundary loss and Codex file-change/usage loss.

### Phase 1 — semantic transcript and neutral renderer

- Define `wg.semantic/v1` and RenderTree v1 with tolerant unknown records.
- Implement independent Pi/Codex/Claude adapters. Pi correlates by ID and folds accumulated updates; no adjacency logic.
- Render tree to Markdown and Ratatui. Keep existing `stream.jsonl` for operational telemetry until all liveness/accounting consumers migrate deliberately.
- Golden tests: semantic JSON, Markdown, narrow/wide TUI snapshots, errors, multi-block/image placeholder, exact cost, and malformed/unknown input.

### Phase 2 — real human-flow TUI gate

- Add a PTY/tmux smoke scenario that opens the actual Log pane on the paired fixture, selects Pretty, and asserts correlated parallel tools, named errors, reasoning, turn separators and usage are visible.
- Assert main currently fails the richer expectation and implementation passes.
- Keep Raw byte-lane regression tests unchanged.

### Phase 3 — Pi session retention and supported HTML cache

- Launch Pi with an attempt-owned `--session-dir` or otherwise copy the exact session identified by the session header into `native/`; never search by “latest”.
- Implement bounded `pi --offline --export` feature probe and atomic cache manifest.
- Tests use a tiny v3 session and a fake Pi executable for absent, old, timeout, crash, malformed output and upgrade cases. When installed Pi is available, an optional live credential-free test exports the fixture; no provider login.
- Verify historical view after removing Pi from PATH and direct Codex view with a PATH trap that fails if `pi` is executed.

### Phase 4 — upstream structured renderer, if available

- Require a documented public package export and protocol handshake.
- Add it as a new optional cache producer; do not parse ANSI/HTML and do not alter native/canonical schemas.

## 9. Acceptance mapping

- Degradation reproduced and localized: §§1.1-1.3.
- Supported APIs distinguished from internals: §2.
- Portable without Pi, Pi-quality enhancement when present: decision + §§4,6-7.
- Direct Codex independent: §§1.3,4,7.
- Drift, tools, errors, usage, history: §§4-6.
- Incremental, credential-free plan: §8.

## References

Repository references are to this checkout. Installed Pi references are rooted at:

`/home/bot/.nvm/versions/node/v25.4.0/lib/node_modules/@earendil-works/pi-coding-agent/`

The installed package reports version 0.82.0 (`package.json:3`) and exposes only its main and RPC entry points (`package.json:14-21`).
