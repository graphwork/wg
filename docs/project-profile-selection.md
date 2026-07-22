# Project-scoped reusable profile selection

Named profile **definitions** remain reusable machine-global files in
`~/.wg/profiles/<name>.toml`. A project may now explicitly select one without
changing another project:

```sh
wg --dir /repo/a/.wg profile select claude
wg --dir /repo/b/.wg profile select codex
wg --dir /repo/a/.wg profile show
wg --dir /repo/a/.wg profile select --clear
```

For an already-installed definition, `profile select` writes only
`<graph>/profile-selection.json` (plus a local, redacted successful-event usage
record). If a built-in starter is not installed yet, apply first materializes
that starter once as the reusable `~/.wg/profiles/<name>.toml` definition with
atomic no-replace semantics; the redacted dry-run plan reports this explicitly.
It does **not** rewrite `~/.wg/config.toml` or `~/.wg/active-profile`. Config
resolution verifies the canonical-project digest and the selected definition's
semantic BLAKE3 fingerprint, then overlays that definition in memory as the
routing authority. The profile is not reconstructed on each run.

The older command remains deliberately global:

```sh
wg profile use claude
```

`profile use` still overlays `~/.wg/config.toml`, updates
`~/.wg/active-profile`, and can hot-reload the current daemon. Its output calls
this global scope out. A global active profile is context only when a project
has an explicit `profile select` association; it is never migrated or presented
as that project's selection.

## Drift and recovery

The association pins profile content. Editing a selected reusable definition
leaves the association explicit but marks it drifted; execution/config loading
fails closed and does not use a global/provider fallback. Inspect and
acknowledge the new routes:

```sh
wg profile show <name>
wg profile select <name>
```

Deleting or renaming a definition likewise leaves existing project
associations unavailable rather than inventing a replacement. Restore the old
definition, select its new name in each project, or clear the association.
Canonical path aliases share one identity; moving a graph requires explicit
re-selection at the new location.

## Read-only catalog and plan APIs

`wg profile list --json` returns installed definitions first, with the current
project selection pinned first. Each entry includes:

- exact handler-first strong, weak, and per-role routes plus reasoning;
- profile source and content fingerprint;
- project association/drift state;
- handler/auth owner, endpoint, and Pi plugin annotations;
- quiet usage labels (`frequent`, `recent`, `used today`, or legacy evidence
  labeled only `recent route`).

`wg profile select <name> --dry-run --json` emits an immutable redacted plan.
Planning and listing write no cache, lock, history, profile, config, or plugin
file. Apply rechecks project, profile, and association preimages before its
atomic write.

Readiness is intentionally conservative. CLI authentication is reported as
`auth status unknown — attended check required`; executable presence is not
called authentication. Endpoint status never exposes a credential reference or
credential path. No readiness failure chooses another handler.

## Local usage history and privacy

Usage ranking reads `${WG_GLOBAL_DIR:-~/.wg}/profile-usage.jsonl`. Records are
created only after successful WorksGood events for a fingerprint-matching
explicit project selection. Each bounded record contains exactly:

- profile name and semantic content fingerprint;
- RFC 3339 timestamp;
- canonical project **digest**, never its path;
- a coarse event category (`profile-selected`, `task-created`,
  `service-started`, or `config-applied`).

Records never contain prompts, endpoint URLs, credentials, credential paths,
raw commands, shell history, or telemetry. Malformed/truncated lines are
ignored, concurrent writers are locked, retention is bounded, and history is
locally inspectable/clearable:

```sh
wg profile history
wg profile history --clear
```

Legacy launcher history is never converted into named-profile usage. An old
launcher model may add the non-attributing label `recent route` only when its
exact canonical route matches a profile.
