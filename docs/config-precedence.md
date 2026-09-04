# Config precedence & the `wg config set/get` surface

`wg config` is the single, complete, **project-local** source of truth for
every dispatcher/profile/registry knob. This doc nails down the precedence
rules so a `wg config set <key> <value>` write reliably **sticks** for the
current repo — surviving `wg service reload`, restart, and the presence of a
reusable profile definition — and so no supported CLI path can invalidate the
profile-origin fingerprint or disable execution.

## Project-local authority (the cutover)

A WG project has **one authoritative configuration document**:

```text
<project-root>/worksgood.toml
```

It is ordinary project source, may be checked in, and is the only persistent
configuration layer consulted for project execution. The decisive precedence
is therefore:

```text
task field > explicit command flag > project worksgood.toml > built-in structural default
```

There is **no global-config position** in that chain. In particular:

- `~/.wg/config.toml` is **not** merged into a project. It remains readable
  only as legacy migration data; project-behavior values read from it are
  labeled `legacy-global (inactive)` and never take effect for a project.
- `~/.wg/active-profile` is **ignored** for project resolution. It is surfaced
  only as legacy inactive state and a migration input. The older "active
  profile is the project route" model is gone; see
  `docs/design-project-local-pi-config.md` §6-7.
- Non-routing global settings do **not** inherit. A missing project key uses a
  documented built-in schema default, reported as `builtin-default` (not
  "inherited").
- Reusable profile **definitions** at `~/.wg/profiles/<name>.toml` are read
  only while planning/applying `wg profile select`; they are never re-read for
  runtime resolution. `profile select` materializes a closed Pi projection
  (model + reasoning for every dispatch role) into `worksgood.toml` and writes
  no global state.

The purpose-scoped `[secrets]` machine namespace is the one exception: the
secret subsystem keeps reading its machine policy directly from
`~/.wg/config.toml`. That is not Config inheritance — it never returns routes
or project behavior, and no secret value or credential path may enter
`worksgood.toml`.

## The generic surface

```sh
wg config set <dotted.toml.key> <value> [--no-reload]
wg config get <dotted.toml.key> [--json]
```

- `set` writes the value into `worksgood.toml` as a **raw TOML tree edit**
  (not a `Config::save` round-trip), so unrelated keys, comments, and unknown
  sections are preserved. Known typed keys (model specs, integer fields) are
  validated up front; the whole document is re-deserialized to confirm it is
  still valid before the write lands. Unknown paths are written as raw TOML so
  **every** knob is reachable without hand-editing files. Global writes
  (`--global`) are refused before mutation with project/profile-definition
  guidance.
- `get` reads the **effective project** value and annotates the winning source
  (`project-file` / `project-profile-import` / `builtin-default` / `unset`, or
  `task` / `command` for override-scope reads).
- `coordinator.*` is accepted as a convenience alias and canonicalized to the
  serde name `dispatcher.*` on write (so the file stays lint-clean — no
  `[coordinator]` deprecation warning on every load).
- Value type is inferred: `true`/`false` → bool, `123` → integer, `1.5` →
  float, anything else → string. (Array/table values use the dedicated
  `--registry-add` / `--tier` paths.)
- Every setter reloads the daemon (a soft `Reconfigure` IPC for non-routing
  keys; a full restart for model/endpoint edits, since running coordinator
  subprocesses keep their spawn-time env) unless `--no-reload` is passed, then
  prints the resolved effective value + its source.

## Routing vs. non-routing under a profile import

When `wg profile select <name>` has materialized a closed Pi projection, the
project document carries `agent.model`, `dispatcher.model`,
`models.<role>.{model,reasoning}`, and a `profile_origin` block. The split:

| Key class | Examples | Authority |
|-----------|----------|-----------|
| **Routing** | `agent.model`, `dispatcher.model`, `[models.*].model`, `[models.*].reasoning` | **Project profile import** (the materialized projection in `worksgood.toml`) |
| **Non-routing tuning** | `dispatcher.max_agents`, `dispatcher.registry_refresh_interval`, `dispatcher.poll_interval`, `[agency].*`, `[dispatcher.resource_management]`, `dispatcher.archive_retention_days`, … | **Explicit project-file write** (the profile projection never imports these) |

### How a direct route edit interacts with a profile import

`wg config set agent.model pi:...` writes to `worksgood.toml` and **atomically
removes `profile_origin`** — the route remains valid project configuration but
is now reported as `project-file (manual)` rather than
`project-profile-import`. A hand edit that changes the projection but leaves
stale origin metadata is detected by the projection fingerprint; inspection
reports `profile-origin-drift`, and LLM execution fails without a global
fallback until the user runs `wg profile select <name>` or
`wg profile select --clear`.

This matches the invariant in the projection writer: the closed Pi allowlist
copies only model + reasoning for every dispatch role plus origin metadata;
non-routing project bytes are preserved semantically and never overwritten by a
profile import.

## Source labels

`Config::load_with_sources` records a source per leaf for the project
document. `wg config --list`, `wg config --show` (the precedence-sensitive
routing/capacity keys carry an inline `[source: …]`), and `wg config get`
surface these labels. Scoped global reads (`config lint --global`,
`config --global --show`) label project-behavior values `legacy-global
(inactive)` and purpose-scoped subsystem namespaces `machine-setting`; they
are never described as project-effective.

## What to use when

| You want to… | Command |
|--------------|---------|
| Tune a non-routing knob for this repo | `wg config set <key> <value>` |
| Set a routing model for this repo (no profile) | `wg config set agent.model pi:...` or `wg setup --route pi --model pi:...` |
| Change routing under a profile import | `wg profile select <name>` / `wg profile select --clear` |
| Disable the OpenRouter registry refresh | `wg config set coordinator.registry_refresh_interval 0` |
| Inspect what won and from where | `wg config get <key>` / `wg config --list` |
| Clean up stale machine-global routing left by an older install | `wg migrate project-local-pi --cleanup-global-routing` |

## Migration from the legacy global model

An older install may have left routing selectors in `~/.wg/config.toml` and an
`~/.wg/active-profile` pointer. These are ignored for project resolution
today. To remove them explicitly while preserving every reusable profile
definition, secret, keystore, identity, and federation byte:

```sh
wg migrate project-local-pi --dry-run        # plan only, writes nothing
wg migrate project-local-pi --cleanup-global-routing --yes
```

See `docs/design-project-local-pi-config.md` §9 and
`docs/project-profile-selection.md`.
