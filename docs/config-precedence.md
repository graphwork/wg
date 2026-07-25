# Config precedence & the `wg config set/get` surface

`wg config` is the single, complete, project-local source of truth for every
dispatcher/profile/registry knob. This doc nails down the precedence rules so a
`wg config set <key> <value> --local` write reliably **sticks** for the current
repo — surviving `wg service reload`, restart, and the presence of an active
profile — and so no supported CLI path can invalidate the profile fingerprint
or disable execution.

## The generic surface

```sh
wg config set <dotted.toml.key> <value> [--local|--global] [--no-reload]
wg config get <dotted.toml.key> [--json]
```

- `set` writes the value into the chosen scope's config file as a **raw TOML
  tree edit** (not a `Config::save` round-trip), so unrelated keys, comments,
  and unknown sections are preserved. Known typed keys (model specs, integer
  fields) are validated up front; the whole document is re-deserialized to
  confirm it is still valid before the write lands. Unknown paths are written
  as raw TOML so **every** knob is reachable without hand-editing files.
- `get` reads the **effective merged** value and annotates the winning source
  (`global` / `local` / `project-profile` / `default`).
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

## Precedence

There are three config layers, merged in this order:

```
~/.wg/config.toml  (global)   ─┐
.wg/config.toml    (local)    ─┴─►  merge_toml  (local wins over global)
                                        │
~/.wg/profiles/<name>.toml     ──────►  overlay_project_profile  (when a project
   (project profile, when                profile association is active)
    selected)                            │
                                        ▼
                                  effective Config
```

The split is **routing vs. non-routing**:

| Key class | Examples | Authority |
|-----------|----------|-----------|
| **Routing** | `agent.model`, `dispatcher.model`, `dispatcher.executor`, `dispatcher.provider`, `[tiers]`, `[models.*]` | **Project profile** (when active) |
| **Non-routing tuning** | `dispatcher.max_agents`, `dispatcher.registry_refresh_interval`, `dispatcher.poll_interval`, `[agency].*`, `[dispatcher.resource_management]`, … | **Explicit local/global config write** (profile supplies only a default) |

### How this is implemented

`profile::named::overlay_project_profile` strips **routing** scalars
(`model`/`executor`/`provider`) and the whole `[tiers]`/`[models]` tables from
the merged global+local tree, then re-overlays them from the profile via
`overlay_profile_for_project`. Non-routing knobs are **not** stripped, and the
sub-table merge (`merge_tables_existing_wins`) **preserves any value the
global/local config already set** — the profile only fills in keys nobody set.
Consequence:

- `wg config set coordinator.max_agents 2 --local` writes `dispatcher.max_agents
  = 2` to `.wg/config.toml`. On the next `wg service reload` (which re-reads the
  merged config), the profile overlay sees the local value already present and
  keeps it — **2 sticks**, where previously the profile's `8` silently reset it
  on every reload.
- `wg config set agent.model pi:...` under an active profile writes to local
  config, but the overlay strips routing keys, so the **profile still wins** for
  routing. To change routing under an active profile, use `wg profile select
  <name>` / `wg profile clear` (or edit the profile via `wg profile set`, which
  re-pins the association fingerprint).

This matches the long-documented invariant in `overlay_project_profile`'s own
comment: *"Non-routing local settings remain intact."* Before this change the
implementation stripped `max_agents` too, contradicting that comment.

### Source labels

`Config::load_with_sources` records a source per leaf, then runs
`refine_sources_by_value` to correct labels by **value-matching** the merged
config against the local and global layers: whichever layer's value actually
survived the overlay is the truthful source. So a locally-overridden
`dispatcher.max_agents` is labeled `local`, while a profile-owned
`agent.model` stays `project-profile`. `wg config --list` / `wg config get`
surface these labels.

## The fingerprint footgun (and how `wg config` avoids it)

The project-profile association pins a `profile_fingerprint` (a semantic BLAKE3
of the profile's TOML content). **Editing the profile file directly** changes
its content fingerprint → the association reports `ContentDrift` → execution is
disabled until `wg profile select <name>` re-acknowledges. That is the footgun.

`wg config set` writes to **`.wg/config.toml`** (local) or `~/.wg/config.toml`
(global) — never to the profile file — so it **cannot** invalidate the profile
fingerprint or disable execution. The only thing that changes a profile's
content is the profile-editing path (`wg profile select` materializes a starter,
`wg profile use` swaps globally, or a future `wg profile set` that re-pins the
fingerprint). Local config edits are safe by construction.

## What to use when

| You want to… | Command |
|--------------|---------|
| Tune a non-routing knob for this repo | `wg config set <key> <value>` (local default) |
| Set a routing model for this repo (no profile) | `wg config set agent.model pi:...` |
| Change routing under an active profile | `wg profile select <name>` / `wg profile clear` |
| Disable the OpenRouter registry refresh | `wg config set coordinator.registry_refresh_interval 0` |
| Inspect what won and from where | `wg config get <key>` / `wg config --list` |
