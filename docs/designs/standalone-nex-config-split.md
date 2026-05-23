# Design: Standalone nex Configuration Split

**Task:** `design-standalone-nex`  
**Date:** 2026-05-23  
**Status:** Proposed

## Summary

`nex` should have a standalone home that is not WG-branded, while `wg nex`
should remain a WG-integrated runtime. The clean split is:

- Standalone `nex` reads and writes human REPL state under `.nex/` or
  `~/.nex/`.
- `wg nex` reads and writes WG-integrated state under `.wg/`, with optional
  `.wg/nex/` overrides for nex-specific project behavior.
- Backward compatibility is read-first and opt-in-write: existing
  `~/.wg/config.toml`, `.wg/config.toml`, `~/.wg/chat/...`, and
  `.wg/chat/...` stay readable; migration commands copy safe subsets with
  backups when the user asks.
- Autonomous WG task execution never reads human `~/.nex` model, endpoint,
  preference, or session state unless a WG config explicitly opts in.

The design introduces an explicit runtime mode instead of deriving behavior from
the shorter binary name:

| Invocation | Runtime mode | Config/state owner |
| --- | --- | --- |
| `nex` | Standalone human | `.nex/` when present, otherwise `~/.nex/` |
| `wg nex` | WG-integrated human | `.wg/`, plus `.wg/nex/` overrides |
| service-spawned native task agent | WG autonomous | `.wg/` only |
| `nex --eval-mode` | Harness/eval | explicit temp/project dir only; no user home state |
| `nex --wg` or legacy `nex --dir <wg>` | WG compatibility | supplied WG dir |

## Existing Shape

Today the standalone `nex` binary is a thin wrapper around the WG resolver:
`src/bin/nex.rs` resolves a WG directory and then calls the same runner used by
`wg nex`. The runner in `src/commands/nex.rs` stores sessions in
`<wg-dir>/chat/<uuid-or-alias>/conversation.jsonl` and
`trace.ndjson`. Config comes from `Config::load_or_default(workgraph_dir)`,
which merges `~/.wg/config.toml` and `<wg-dir>/config.toml`. Sessions are
registered through `src/chat_sessions.rs` under `<wg-dir>/chat/sessions.json`.

That is correct for `wg nex`, coordinator chats, and task-agent sessions. It is
confusing for standalone human use because running `nex` from a random terminal
creates or reuses WG-branded state.

## Directory Layout

### Standalone `nex`

`nex` uses two concepts:

- **Project nex dir**: the nearest `.nex/` found by walking up from cwd, or the
  path passed by `--nex-dir` / `NEX_DIR`.
- **User nex home**: `NEX_HOME`, else `~/.nex/`.

Fresh standalone sessions write to the project nex dir when one exists. If no
`.nex/` exists, fresh sessions write to `~/.nex/` instead of creating `.nex/` in
an arbitrary cwd. `nex init` creates project `.nex/`.

Recommended layout:

```text
~/.nex/
  config.toml              # standalone user defaults and preferences
  models.yaml              # user model aliases/registry
  sessions/
    <uuid>/
      conversation.jsonl   # replayable journal
      trace.ndjson         # low-level nex event log
      stream.jsonl         # structured turn/tool telemetry, when enabled
      inbox.jsonl          # only for chat-tethered/server modes
      outbox.jsonl         # only for chat-tethered/server modes
      .streaming           # ephemeral current assistant text
      .handler.pid         # single-handler lock
      checkpoints/
      tool-outputs/
      fetched-pages/
  cache/
    models/
    web/
    mcp/
    tokenizer/
  keystore/                # secure file backend when explicitly selected
```

```text
<project>/.nex/
  config.toml              # project overrides for standalone nex
  models.yaml              # project model aliases/metadata
  sessions/                # project-local human sessions
  cache/                   # project-local cache entries safe to delete
```

`~/.nex/config.toml` should reuse shared WG config blocks where useful, but it
is not the full WG schema. Supported standalone blocks are:

- `[nex]` for runtime and UI preferences
- `[[llm_endpoints.endpoints]]`
- `[[model_registry]]` or `models.yaml`
- `[models.*]` and `[tiers]` for role/model aliases used by nex
- `[native_executor]`
- `[[mcp.servers]]`
- `[secrets]`

WG-only blocks such as `[dispatcher]`, `[agency]`, `[project]`, and
`[[tag_routing]]` are ignored with `nex config lint` warnings when found in
`~/.nex/config.toml`.

### WG-integrated `wg nex`

WG-integrated sessions keep using WG storage because they are graph-visible and
must interoperate with TUI chat, session locks, coordinator respawn, and task
recovery:

```text
<project>/.wg/
  config.toml
  models.yaml
  chat/
    sessions.json
    <uuid>/
      conversation.jsonl
      trace.ndjson
      stream.jsonl
      inbox.jsonl
      outbox.jsonl
      .streaming
      .handler.pid
  nex/
    config.toml            # nex-specific WG project overrides
    cache/
```

`.wg/nex/config.toml` is for nex-specific runtime choices that should not
pollute general WG config: human REPL defaults, tool-surface presets, prompt
snippets, and optional project-local endpoint aliases. Dispatcher-critical
routing remains in `.wg/config.toml`.

## Precedence Rules

All precedence is highest to lowest.

### Standalone `nex`: Config and Preferences

1. CLI flags: `--model`, `--endpoint`, `--system-prompt`, `--read-only`,
   `--minimal-tools`, `--nex-dir`, `--config`
2. `NEX_*` environment variables: `NEX_MODEL`, `NEX_ENDPOINT`, `NEX_HOME`,
   `NEX_DIR`, `NEX_CONFIG`, `NEX_STREAM_IDLE_TIMEOUT_SECS`
3. Project `.nex/config.toml`
4. User `~/.nex/config.toml`
5. Read-only imported WG compatibility source: when cwd is inside a WG
   project, `<project>/.wg/config.toml`, then `~/.wg/config.toml`
6. Built-in defaults

`WG_MODEL` and `WG_STREAM_IDLE_TIMEOUT_SECS` remain deprecated compatibility
fallbacks for standalone `nex` only when the corresponding `NEX_*` variable is
unset. `WG_DIR` is not a standalone config home; `nex --dir <path>` is retained
as a legacy alias for WG compatibility mode.

### Standalone `nex`: Endpoints

1. CLI `--endpoint`: named endpoint or URL
2. `NEX_ENDPOINT`
3. Project `.nex/config.toml` endpoint table
4. User `~/.nex/config.toml` endpoint table
5. WG compatibility endpoint tables from `.wg/config.toml` and
   `~/.wg/config.toml`
6. Provider defaults, such as local OpenAI-compatible defaults

Endpoint arrays merge by `name`; the higher layer replaces the lower endpoint
with the same name. The default endpoint is the highest layer that declares
`is_default = true`; if none does, provider/model matching chooses first and
then built-ins.

Inline URL endpoints from CLI remain zero-config and do not read implicit API
key environment variables.

### Standalone `nex`: Model Aliases and Registry

1. CLI `--model`
2. `NEX_MODEL`
3. Project `.nex/models.yaml`, `[[model_registry]]`, `[models.*]`, `[tiers]`
4. User `~/.nex/models.yaml`, `[[model_registry]]`, `[models.*]`, `[tiers]`
5. WG compatibility registry and model blocks
6. Built-in registry

Higher layers replace registry entries by model `id`. Bare aliases are resolved
after endpoint selection so `-m qwen3-coder -e lambda01` continues to mean the
wire model `qwen3-coder` on endpoint `lambda01`.

### Standalone `nex`: Secrets

Secret values are user credentials, not project state. Config files should
reference credentials; they should not duplicate plaintext keys.

Resolution:

1. Explicit future CLI `--api-key-ref` / `--api-key-file` if added. Avoid a raw
   `--api-key` except for test-only paths.
2. Matched endpoint fields in config: `api_key_ref`, then `api_key_file`, then
   deprecated `api_key_env`, then inline `api_key` only if already present.
3. Secret ref resolver:
   - `op://...`, `pass:...`, `env:VAR`, `literal:...`
   - `keyring:<name>` uses the existing WG keyring service for v1 so existing
     keys keep working.
   - `keystore:<name>` reads `~/.nex/keystore` first for standalone, then
     legacy `~/.wg/keystore` for compatibility.
   - `plain:<name>` is disabled unless `[secrets].allow_plaintext = true`; new
     standalone plaintext writes go to `~/.nex/secrets`, but migration should
     avoid creating them.

No provider-specific implicit env fallback is added. If a user wants env-based
secrets, they write `api_key_ref = "env:OPENROUTER_API_KEY"` or the deprecated
`api_key_env = "OPENROUTER_API_KEY"` explicitly.

### Standalone `nex`: Sessions, Tool Outputs, Checkpoints, Caches

Fresh session root:

1. CLI `--session-dir` if added
2. Project `.nex/sessions/` when a project `.nex/` exists
3. User `~/.nex/sessions/`

Resume search:

1. Explicit `--chat <ref>` / future `--session <ref>`
2. Project `.nex/sessions/`
3. User `~/.nex/sessions/`
4. Legacy `<project>/.wg/chat/` and `~/.wg/chat/`

When a legacy session is resumed, writes stay in that legacy session directory
for compatibility. `nex migrate sessions` copies sessions into the new layout
when the user asks.

Tool outputs, fetched pages, and checkpoints are session-scoped:

- `<session>/tool-outputs/`
- `<session>/fetched-pages/`
- `<session>/checkpoints/`

Cache lookup:

1. CLI/env cache override if added
2. Project `.nex/cache/`
3. User `~/.nex/cache/`
4. Legacy WG cache locations as read-only compatibility where applicable
5. Rebuild/refetch

Caches must never be the source of authoritative model, endpoint, or credential
configuration.

### `wg nex`: Human Integrated Precedence

1. CLI flags passed to `wg nex`
2. WG runtime environment and per-chat overrides: `WG_MODEL`, explicit
   endpoint override, chat metadata
3. Project `.wg/nex/config.toml`
4. Project `.wg/config.toml`
5. User `~/.wg/config.toml` and active WG profile snapshot
6. User `~/.nex/config.toml` for human interactive preferences only
   (theme, prompt snippets, display density). Model and endpoint blocks from
   `~/.nex` are ignored unless `.wg/nex/config.toml` sets
   `inherit_standalone_config = true`.
7. Built-in defaults

`wg nex` resumes from `.wg/chat/` only. It does not silently mix standalone
`~/.nex/sessions` into `wg nex --resume`; use an explicit import/fork command
if a standalone session should become a WG chat session.

### WG Autonomous / Task-Agent Precedence

1. Spawn CLI args and controlled WG env from the dispatcher
2. Task fields and per-chat/session metadata
3. Project `.wg/nex/config.toml` keys marked `apply_to_autonomous = true`
4. Project `.wg/config.toml`
5. User `~/.wg/config.toml` and active profile snapshot
6. Built-in defaults

Autonomous mode never reads `~/.nex/config.toml`, `~/.nex/models.yaml`,
`~/.nex/sessions`, or `~/.nex/cache`. The only shared user-level resource is the
credential resolver for explicit `api_key_ref` values.

Detect autonomous mode by any of:

- `--autonomous`
- `--eval-mode`
- `WG_TASK_ID` present
- `WG_AGENT_ID` present
- chat/session metadata identifies a task-agent handler

`--eval-mode` should be stricter: it uses only explicit CLI/env and the supplied
project/temp config, skips user `~/.wg` and `~/.nex` unless the harness passes an
opt-in flag, and keeps the existing no-chat-surface behavior.

## Command Surface

Recommendation: `nex` should have its own UX, implemented by reusing WG internals
rather than by telling users to run `wg endpoints`.

Staged command surface:

### Stage 1: Minimum Standalone Completeness

- `nex setup`
  - Creates `~/.nex/config.toml` or project `.nex/config.toml`
  - Offers routes: local/OAI-compatible, OpenRouter, Anthropic direct, OpenAI
  - Offers `--import-wg` to copy safe endpoint/model blocks from WG config
- `nex init`
  - Creates project `.nex/` without running the full setup wizard
- `nex endpoints list|add|update|remove|set-default|test`
  - Same endpoint semantics as WG, targeting `.nex`/`~/.nex`
- `nex config show|lint|path`
- `nex sessions list|show|fork|archive`
  - Lists new and legacy sessions, marking legacy clearly
- `nex migrate config --from-wg --dry-run`
- `nex migrate sessions --from-wg --dry-run`

### Stage 2: Credential and Model Parity

- `nex secret set|get|list|rm|check|backend`
  - Same backend implementation as `wg secret`
  - `keyring:` remains shared for v1; file backends are standalone-first with
    legacy read fallback
- `nex models list|search|add|set-default|fetch`
  - Targets `.nex/models.yaml` / `~/.nex/models.yaml`

### Stage 3: Deprecate WG-Centric Standalone Guidance

- Standalone help should stop saying "run `wg endpoints`" except in a legacy
  compatibility note.
- `nex --dir` warning: "using WG compatibility home; prefer `wg nex` for WG
  sessions or `nex --nex-dir` for standalone sessions."

## Migration Strategy

Migration must be conservative because current users may have valuable sessions
and hand-edited endpoint config under `~/.wg`.

### Default Behavior on Upgrade

- `nex` fresh sessions write to `.nex`/`~/.nex`.
- `nex --resume` searches new locations first, then legacy WG session locations.
- Resuming a legacy session writes back to the legacy directory.
- `wg nex --resume` remains WG-scoped and continues to find existing WG sessions.
- No existing `.wg` or `~/.wg` file is rewritten automatically.

### Config Migration

`nex migrate config --from-wg`:

1. Reads `~/.wg/config.toml` and, optionally, the nearest project
   `.wg/config.toml`.
2. Copies only standalone-relevant blocks: endpoints, model aliases/registry,
   native executor config, MCP servers, and secrets settings.
3. Drops dispatcher, agency, graph, profile-pointer, tag routing, project, TUI,
   and WG chat-only sections.
4. Preserves `api_key_ref` and explicit `api_key_env`.
5. Does not copy inline `api_key` by default. It emits a migration note and
   suggests `nex secret set <name>` or `api_key_ref = "env:VAR"`. A
   `--copy-inline-secrets` escape hatch can exist for local/private machines,
   but it should be noisy.
6. Backs up any existing destination:
   `~/.nex/config.toml.bak-<utc-timestamp>`.

`nex setup --import-wg` should use the same migration engine.

### Session Migration

`nex migrate sessions --from-wg`:

1. Scans `~/.wg/chat/` and nearest project `.wg/chat/`.
2. Reads `sessions.json` when available; also handles legacy numeric
   directories.
3. Copies selected sessions to `~/.nex/sessions/<uuid>/` or
   `.nex/sessions/<uuid>/`.
4. Preserves `conversation.jsonl`, summaries, trace logs, and fetched/tool
   artifacts.
5. Does not move or delete legacy sessions.
6. Writes an alias map so `nex --resume <old-alias>` can find the migrated
   session.

### Secrets Migration

Do not duplicate plaintext secrets automatically.

- `keyring:<name>` works without migration because v1 shares the existing keyring
  service.
- `keystore:<name>` may be copied from `~/.wg/keystore` to `~/.nex/keystore`
  only by explicit `nex migrate secrets --from-wg`.
- `plain:<name>` is reported but not copied unless the user passes an explicit
  plaintext opt-in.
- Inline endpoint `api_key = "..."`
  should be converted to a suggested `api_key_ref` flow, not silently copied.

## Implementation Phases

### Phase 1: Runtime Home Resolver

Add a `NexRuntimeMode` and `NexHome` resolver shared by `nex` and `wg nex`.
The resolver returns config roots, state roots, session roots, cache roots, and
legacy read roots. Unit tests pin every mode.

This phase should preserve current `wg nex` behavior while changing fresh
standalone `nex` writes to `.nex`/`~/.nex`.

### Phase 2: Config Loader Split

Create a standalone nex config loader that understands the supported subset and
shared endpoint/model blocks. Keep WG `Config::load_merged` for WG-integrated
mode. Provider creation should receive an effective endpoint/model registry
rather than always reloading WG config by `workgraph_dir`.

### Phase 3: Session Search and Migration

Generalize session registry paths from "chat under WG dir" to an injected
session root. Preserve WG chat layout for WG mode. Add legacy search and
explicit migration commands.

### Phase 4: Standalone Commands

Add `nex setup`, `nex endpoints`, `nex config`, `nex sessions`, and migration
commands. Reuse endpoint, model, and secret internals, but target nex homes.

### Phase 5: Tests, Smoke, Docs

Add integration tests for precedence and smoke coverage for the user-visible
flows:

- Fresh `nex` does not create `~/.wg` or `.wg`.
- `nex --resume` can resume a legacy WG session.
- `wg nex` ignores `~/.nex` model/endpoint config in autonomous mode.
- `nex setup --import-wg --dry-run` does not copy inline plaintext keys.

## Follow-up WG Tasks

Created from this design:

- `implement-standalone-nex`: source implementation and migration behavior.
- `verify-standalone-nex`: integration and smoke coverage.
- `docs-standalone-nex`: user-facing setup and migration documentation.

## Rejected Alternatives

### Keep `nex` as a Thin Alias for `wg nex`

Rejected. It preserves current implementation simplicity but contradicts the
product direction: standalone `nex` should feel complete and should not make
users learn WG storage conventions for basic REPL use.

### Auto-Migrate All Existing WG State on First Run

Rejected. Auto-copying config and sessions creates duplicate mutable state and
risks duplicating plaintext API keys. Read compatibility plus explicit migration
with backups is safer.

### Let `wg nex` Freely Read `~/.nex`

Rejected for autonomous execution. Human `wg nex` can optionally borrow
standalone preferences, but task agents must remain deterministic and graph
controlled.

## Validation Checklist

- Directly answers ownership: standalone state goes under `.nex`/`~/.nex`; WG
  integrated state stays under `.wg`.
- Gives explicit precedence for config, endpoints, model aliases, secrets,
  sessions, tool outputs, and caches.
- Preserves existing WG config, secrets, and sessions through read compatibility.
- Recommends standalone command surface with staged implementation.
- Prevents autonomous WG task agents from reading human `~/.nex` state.
