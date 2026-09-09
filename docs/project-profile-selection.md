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

`profile select` is a **project-local, copy-by-value** operation. It resolves
an isolated, closed Pi projection (model + reasoning for every dispatch
role) from one definition — with no global/local config inputs — and writes
that projection into `<project-root>/worksgood.toml`, the single authoritative
project document. It writes **only** `worksgood.toml` (plus a redacted
successful-event usage record in `~/.wg/profile-usage.jsonl`). If a built-in
starter is not installed yet, apply first materializes that starter once as
the reusable `~/.wg/profiles/<name>.toml` definition with atomic no-replace
semantics; the redacted dry-run plan reports this explicitly. It does **not**
rewrite `~/.wg/config.toml` or `~/.wg/active-profile`, never edits `~/.pi`,
and never re-reads the definition at runtime. A later edit, rename, or
deletion of the reusable definition therefore cannot change or disable an
already-configured project; re-running `profile select` is the explicit update
operation.

The older global verb is a deprecated alias:

```sh
wg profile use claude     # [DEPRECATED] alias for project-local `profile select`
```

`profile use` now performs the same project-local materialization into
`worksgood.toml` when a project root is available (it warns once and never
performs the old global mutation). Without project context it fails with
actionable guidance. `~/.wg/active-profile` is **ignored** for project
resolution — it is surfaced only as legacy inactive state and a migration
input, never as a project's selected route.

## Drift and recovery

The materialized projection records `profile_origin` with the definition and
projection fingerprints. Editing a selected reusable definition does **not**
affect the project at runtime (the bytes are already in `worksgood.toml`);
however, `profile show` reports the definition as unavailable/drifted. To
adopt a changed definition's new routes, re-run `wg profile select <name>`.

A hand edit that changes a managed route but leaves stale `profile_origin`
metadata is detected by the projection fingerprint: inspection reports
`profile-origin-drift` and LLM execution fails without a global fallback.
Recover by running `wg profile select <name>` (re-materialize) or
`wg profile select --clear` (keep the current routes, drop only origin
metadata, source becomes `project-file (manual)`).

Deleting or renaming a definition likewise leaves `profile show` reporting the
definition unavailable; the project route itself keeps working because the
projection is copy-by-value. Re-select at the new name, or clear the origin.
Clone portability comes from the relative project-root binding and the
checked-in `worksgood.toml`, not a canonical-path digest: a clone reproduces
the exact route/reasoning without the original machine's profiles or
credentials.

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
