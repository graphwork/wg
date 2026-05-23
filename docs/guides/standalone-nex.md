# Standalone nex Setup and Migration

`nex` and `wg nex` use the same native agent runtime, but they do not own the
same config and state.

Use `nex` when you want a human REPL that is independent of a WG task graph:
experiments, local coding help, model shakedowns, or project-local sessions
that should live in `.nex/`.

Use `wg nex` when the session belongs to a WG project: TUI chat sessions,
coordinator sessions, task-agent sessions, or any run that should use WG
routing, profiles, logs, and `.wg/chat/` state.

Autonomous WG agents do not read human `~/.nex` routing state. A task spawned
by the WG dispatcher gets model and endpoint choices from WG config and task
metadata, not from your personal standalone `nex` preferences.

## First Run: Standalone `nex`

The standalone path is for a personal REPL, not for dispatching WG tasks.

```bash
# Interactive first-run wizard for user-wide standalone defaults.
nex setup

# Start a fresh standalone session. A bare `nex` invocation does not
# auto-resume an old session.
nex
```

For a project-local standalone setup, run setup from the project root and choose
the project `.nex/` target when prompted:

```bash
cd your-project
nex setup --project
nex
```

Non-interactive examples follow the same route shape as `wg setup`, but write to
`~/.nex/` or `.nex/` instead of `~/.wg/` or `.wg/`:

```bash
# Local OpenAI-compatible server, such as Ollama, vLLM, or llama.cpp.
nex setup --route local \
  --url http://localhost:11434/v1 \
  --model qwen3:4b \
  --yes

# OpenRouter using an explicit environment-backed key reference.
export OPENROUTER_API_KEY=...
nex setup --route openrouter \
  --model openrouter:qwen/qwen3-coder \
  --api-key-ref env:OPENROUTER_API_KEY \
  --yes
```

If your installed build already has the runtime split but does not yet expose
the standalone setup subcommands, use the equivalent file layout directly:

```bash
mkdir -p ~/.nex
cat > ~/.nex/config.toml <<'TOML'
[models.task_agent]
model = "nex:qwen3:4b"

[[llm_endpoints.endpoints]]
name = "local"
provider = "openai"
url = "http://localhost:11434/v1"
is_default = true
TOML

nex
```

Create a project `.nex/` only when you want sessions and config for that
repository to stay with that repository:

```bash
mkdir -p .nex
cp ~/.nex/config.toml .nex/config.toml
nex
```

## First Run: WG-Integrated `wg nex`

The WG-integrated path keeps the session inside the WG project so the TUI,
service daemon, and coordinator can see and resume it.

```bash
cd your-project
wg init

# Pick a WG route. This writes WG config, not standalone nex config.
wg setup --route local \
  --url http://localhost:11434/v1 \
  --model qwen3:4b \
  --yes

# Start an integrated human nex session.
wg nex
```

You can also use an explicit model and endpoint for one run:

```bash
wg nex -m nex:qwen3-coder -e http://127.0.0.1:8088
```

Use `.wg/nex/config.toml` for nex-specific WG project overrides that should not
go in the general dispatcher config:

```bash
mkdir -p .wg/nex
cat > .wg/nex/config.toml <<'TOML'
[models.task_agent]
model = "nex:qwen3-coder"

[[llm_endpoints.endpoints]]
name = "project-local"
provider = "openai"
url = "http://127.0.0.1:8088/v1"
is_default = true
TOML

wg nex
```

By default `.wg/nex/config.toml` is for human `wg nex` sessions. Autonomous WG
task agents only read `.wg/nex/config.toml` when that file explicitly opts in:

```toml
[nex]
apply_to_autonomous = true
```

Do not use `wg endpoints` as the only way to manage standalone `nex`.
`wg endpoints` changes WG config. Standalone `nex` should use `nex setup`,
`nex endpoints`, or the `.nex/config.toml` file layout above.

## Directory Ownership

| Path | Owner | Purpose |
| --- | --- | --- |
| `.nex/` | Standalone `nex` for the current project | Project-local standalone config, model aliases, sessions, and cache. If this directory exists above the current working directory, bare `nex` writes fresh sessions here. |
| `~/.nex/` | Standalone `nex` for the current user | User-wide standalone defaults and sessions. Bare `nex` uses this when no project `.nex/` is found. |
| `.wg/nex/` | WG-integrated `wg nex` | Nex-specific overlay config and cache for a WG project. It is not the WG chat session store. |
| `.wg/chat/` | WG chat/session runtime | Session registry, conversation journals, inbox/outbox files, streaming files, and handler locks for `wg nex`, TUI chat, coordinator sessions, and task agents. |
| `~/.wg/` and `.wg/` | WG itself | WG global and project config, profiles, task graph state, service state, and legacy compatibility inputs for standalone `nex`. |

The important boundary is that standalone sessions live under `.nex/sessions/`
or `~/.nex/sessions/`, while WG-integrated sessions live under `.wg/chat/`.
`wg nex --resume` searches WG chat sessions only. It does not silently mix in
`~/.nex/sessions/`.

Older WG-native tools may also have written artifacts under `.wg/nex-sessions/`
such as fetched pages or pending tool buffers. Treat those as legacy artifacts:
keep them until you have migrated or archived the related sessions, but do not
use that directory as the standalone `nex` session home.

## Standalone Precedence

Standalone `nex` uses a small compatibility layer so existing WG endpoint and
model config can still be read, but `.nex` owns new standalone state.

Direct overrides are highest precedence:

1. CLI flags such as `--model`, `--endpoint`, `--system-prompt`,
   `--read-only`, and `--minimal-tools`.
2. Standalone environment variables such as `NEX_MODEL`, `NEX_ENDPOINT`, and
   `NEX_STREAM_IDLE_TIMEOUT_SECS`.

After direct overrides, config files merge in this order, highest precedence
first:

1. Extra config passed with `--config` or `NEX_CONFIG`.
2. Project `.nex/config.toml`, if a project `.nex/` is found.
3. User `~/.nex/config.toml`.
4. Compatibility WG config from the nearest project `.wg/config.toml`.
5. Compatibility WG config from `~/.wg/config.toml`.
6. Built-in defaults.

For model selection, CLI `--model` wins over `NEX_MODEL`, which wins over the
merged config. `WG_MODEL` is only a compatibility fallback for standalone `nex`
when `NEX_MODEL` is unset.

For endpoint selection, CLI `--endpoint` wins over `NEX_ENDPOINT`, which wins
over endpoint tables in the merged config. The endpoint can be either a named
endpoint or a direct URL.

Endpoint arrays merge by `name`; a higher-precedence endpoint with the same
name replaces the lower-precedence endpoint. Model registry entries merge by
model id.

Secrets are resolved only when an endpoint asks for them:

1. Prefer `api_key_ref`, for example `api_key_ref = "env:OPENROUTER_API_KEY"`
   or `api_key_ref = "keyring:openrouter"`.
2. `api_key_file` and deprecated `api_key_env` are still accepted when already
   present in migrated config.
3. Inline `api_key = "..."` is plaintext and should not be used in new config.

Standalone sessions use one active state root:

1. `--nex-dir <path>` if passed.
2. `NEX_DIR` if set.
3. The nearest `.nex/` found by walking up from the current directory.
4. `NEX_HOME`, otherwise `~/.nex/`.

Fresh sessions are written to `<active-state-root>/sessions/`. Resume searches
that active standalone session root first, then legacy WG chat roots for
compatibility. If you are inside a project with `.nex/` but want to resume a
user-home standalone session, point at it explicitly:

```bash
nex --nex-dir ~/.nex --resume
```

## WG-Integrated Precedence

Human `wg nex` sessions use WG state and WG routing:

1. CLI flags passed to `wg nex`.
2. WG runtime environment and per-chat metadata.
3. Project `.wg/nex/config.toml`.
4. Project `.wg/config.toml`.
5. User `~/.wg/config.toml` and active WG profile.
6. Built-in defaults.

Human `wg nex` does not read `~/.nex` model or endpoint blocks unless the WG
overlay explicitly opts in:

```toml
[nex]
inherit_standalone_config = true
```

Autonomous task-agent `wg nex` runs are stricter:

1. Dispatcher-supplied CLI args and controlled WG environment.
2. Task fields and per-session metadata.
3. Project `.wg/nex/config.toml` only when `[nex].apply_to_autonomous = true`.
4. Project `.wg/config.toml`.
5. User `~/.wg/config.toml` and active WG profile.
6. Built-in defaults.

They never read `~/.nex/config.toml`, `~/.nex/models.yaml`,
`~/.nex/sessions/`, or `~/.nex/cache/`.

## Safe Migration From WG State

Migration is explicit. No upgrade should rewrite or delete existing `~/.wg/` or
`.wg/` files automatically.

Preview config migration before writing:

```bash
# Copy safe standalone-relevant blocks from user WG config.
nex migrate config --from-wg --global --dry-run

# Copy safe standalone-relevant blocks from this project's WG config.
nex migrate config --from-wg --local --dry-run
```

Apply after reading the preview:

```bash
nex migrate config --from-wg --global
nex migrate config --from-wg --local --project
```

If your installed build has the runtime split but not the standalone migration
subcommands yet, do the same migration manually and conservatively: create the
destination `.nex/config.toml` or `~/.nex/config.toml`, copy only endpoint,
model, native executor, MCP, and secret-setting blocks from the WG config, and
leave the original `~/.wg/config.toml` or `.wg/config.toml` untouched.

The config migration copies only standalone-relevant sections such as endpoint
tables, model aliases and registry entries, native executor settings, MCP
servers, and secret settings. It drops dispatcher, agency, service, task graph,
TUI, project metadata, and other WG-only sections. Existing destination files
are backed up first, for example `~/.nex/config.toml.bak-<timestamp>`.

Plaintext keys are not auto-copied. `api_key_ref` and explicit `api_key_env`
entries are preserved, but inline endpoint keys like this are refused by
default:

```toml
api_key = "sk-plaintext"
```

Move plaintext secrets to a reference before migrating:

```bash
# Environment-backed reference.
export OPENROUTER_API_KEY=...
# Then set the endpoint to use: api_key_ref = "env:OPENROUTER_API_KEY"

# Or store it through the secret backend.
nex secret set openrouter
# Then set the endpoint to use: api_key_ref = "keyring:openrouter"
```

Preview session migration separately:

```bash
# User-level legacy WG chat sessions.
nex migrate sessions --from-wg --global --dry-run

# Project-level legacy WG chat sessions.
nex migrate sessions --from-wg --local --dry-run
```

The session migration scans `~/.wg/chat/` and the nearest project
`.wg/chat/`, including `sessions.json` when present. It copies selected
sessions into `~/.nex/sessions/` or `.nex/sessions/`, preserving
`conversation.jsonl`, trace logs, summaries, fetched pages, tool outputs, and
aliases where available. It does not move or delete the legacy WG chat
directories.

If you have very old artifacts under `.wg/nex-sessions/`, keep that directory
until you have confirmed the migrated session still has the fetched pages and
tool outputs you need.

On split-only builds without `nex migrate sessions`, the safe manual approach is
to copy, not move, the session directories you still need:

```bash
SESSION_UUID=...

mkdir -p ~/.nex/sessions
cp -a "$HOME/.wg/chat/$SESSION_UUID" "$HOME/.nex/sessions/$SESSION_UUID"

mkdir -p .nex/sessions
cp -a ".wg/chat/$SESSION_UUID" ".nex/sessions/$SESSION_UUID"
```

Keep `~/.wg/chat/`, `.wg/chat/`, and `.wg/nex-sessions/` until you have opened
the migrated sessions and confirmed that the conversation journals and related
tool artifacts are present.

## Quick Checks

Use these commands to confirm which side owns a session:

```bash
# Standalone state roots.
find .nex ~/.nex -path '*/sessions/*/conversation.jsonl' -print 2>/dev/null

# WG-integrated state root.
find .wg/chat -name conversation.jsonl -print 2>/dev/null
```

Use `wg nex` for `.wg/chat/` sessions. Use `nex` for `.nex/` and `~/.nex/`
sessions.
