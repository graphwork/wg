# Design: `wg login openrouter`

## Goal

Make the common OpenRouter setup path as simple as Pi's provider login flow:

```bash
wg login openrouter
wg login openrouter --check
wg model-scout
wg profile pi
```

The user should not need to understand `wg secret`, `api_key_ref`, endpoint
stanzas, or `wg migrate secrets` to get a working OpenRouter-backed WG setup.
Those lower-level surfaces remain for advanced/manual use, but the recommended
path becomes a single obvious command.

## Pi surfaces confirmed

### Local CLI / local config

Observed from the installed `pi` binary and local `~/.pi/agent/*` state on
2026-07-04:

- `pi --help` exposes direct runtime auth knobs: `--provider`, `--model`, and
  `--api-key`, plus documented provider env-var fallbacks including
  `OPENROUTER_API_KEY`.
- `pi config` is a TUI for enabling/disabling package resources, not the auth
  entry point.
- `~/.pi/agent/auth.json` stores provider credentials keyed by provider name.
  In the current install it contains:
  - an OAuth-shaped entry for `openai-codex`
  - an API-key-shaped entry for `openrouter`
- `~/.pi/agent/models.json` stores custom provider/model definitions. In the
  current install it contains a provider object with `baseUrl`, `api`,
  `apiKey`, `headers`, `compat`, and `models`.

No secret values are reproduced here.

### Public Pi docs

Pi's public docs match the local behavior:

- Providers docs: `/login` can store API-key-provider credentials in
  `~/.pi/agent/auth.json`, and environment variables are also supported.
  Source: https://pi.dev/docs/latest/providers
- Quickstart docs: users can either export an API key before launch or use
  `/login` and select an API-key provider. Source:
  https://pi.dev/docs/latest/quickstart
- Custom models docs: provider/model entries in `models.json` can include
  `baseUrl`, `api`, `apiKey`, and `headers`; models may be present but remain
  unavailable until auth is configured. Source:
  https://pi.dev/docs/latest/models
- Settings docs: `~/.pi/agent/settings.json` is Pi's global settings surface,
  separate from auth. Source: https://pi.dev/docs/latest/settings

## WG problem statement

WG already has the underlying pieces, but the user-facing setup is fragmented:

- secure secret storage: `wg secret set ...`
- endpoint creation: `wg endpoints add ...`
- legacy migration: `wg migrate secrets`
- old shorthand path: `wg key set openrouter`

That means the current answer to "how do I use OpenRouter?" is procedural
knowledge, not a product surface.

The key architectural constraint is that WG has two distinct OpenRouter paths:

1. `openrouter:...` and `nex:openrouter:...`
   WG itself is the HTTP client and therefore needs a WG-managed key.
   Existing code already resolves this from endpoint config and secret/env
   fallback. See [src/config.rs](/home/bot/wg/src/config.rs:1011),
   [src/commands/endpoints.rs](/home/bot/wg/src/commands/endpoints.rs:1), and
   [src/commands/key.rs](/home/bot/wg/src/commands/key.rs:1).

2. `pi:openrouter/...`
   Pi is the provider client and can run on Pi-managed auth, but WG's
   `pi-handler` can also inject a WG-managed key into Pi's environment when WG
   has one configured. See [src/commands/pi_handler.rs](/home/bot/wg/src/commands/pi_handler.rs:100)
   and [src/commands/pi_handler.rs](/home/bot/wg/src/commands/pi_handler.rs:960).

The Pi profile already encodes this split correctly:

- strong tier persists as `pi:openrouter/...` so Pi can self-auth for workers
- weak tier stays `openrouter:...` so WG-native one-shot agency calls use WG's
  own OpenRouter path

See [src/config.rs](/home/bot/wg/src/config.rs:2599),
[src/profile/templates/pi.toml](/home/bot/wg/src/profile/templates/pi.toml:1),
and [src/model_scout.rs](/home/bot/wg/src/model_scout.rs:148).

## Proposed UX

### Primary command

Add a top-level login surface:

```bash
wg login openrouter
```

Behavior:

1. Prompt for the API key without echo.
2. Store it in WG's secret backend under a canonical name:
   `keyring:openrouter` when keyring is reachable, otherwise the configured
   default secure WG backend.
3. Ensure a canonical endpoint exists in global config:
   - `name = "openrouter"`
   - `provider = "openrouter"`
   - `url = "https://openrouter.ai/api/v1"`
   - `api_key_ref = "keyring:openrouter"` or equivalent backend ref
4. If an `openrouter` endpoint already exists, patch only the auth-related
   fields unless `--reset-endpoint` is explicitly requested.
5. Mark that endpoint default only when there is no existing default endpoint,
   or when the user passes `--set-default`.
6. Print a short next-step summary:
   - `wg login openrouter --check`
   - `wg model-scout`
   - `wg profile pi`

Recommended non-interactive form:

```bash
printf '%s' "$OPENROUTER_API_KEY" | wg login openrouter --from-stdin
```

Intentionally do not recommend `wg login openrouter --api-key ...`; keys must
not travel in argv or shell history.

### Check / status flow

Add a check mode:

```bash
wg login openrouter --check
```

Output should be terse and actionable:

- whether WG has a stored/reachable secret ref for OpenRouter
- which endpoint name/url WG will use for native OpenRouter traffic
- whether that endpoint is default
- whether Pi has its own OpenRouter login
- exact next action when something is missing

Example shape:

```text
OpenRouter (WG)
  secret: present (keyring:openrouter)
  endpoint: openrouter -> https://openrouter.ai/api/v1
  default: yes

OpenRouter (Pi)
  auth: present in ~/.pi/agent/auth.json

Next:
  wg model-scout
  wg profile pi
```

If Pi auth cannot be detected, say so explicitly but do not fail the WG login
check:

```text
OpenRouter (Pi)
  auth: not detected
  note: `pi:` routes can still work later if you run `/login` inside pi
```

### Optional explicit helpers

The first implementation can stop at `wg login openrouter` + `--check`. If
follow-up surface area is acceptable, add:

```bash
wg login list
wg login logout openrouter
```

But these are not required for the initial user-pain fix.

## Command semantics and boundaries

### What `wg login openrouter` configures

It configures the WG-native OpenRouter path used by:

- `openrouter:...`
- `nex:openrouter:...`
- weak-tier agency calls when the active profile resolves them to native
  OpenRouter
- `pi-handler` environment injection when WG chooses to pass its own OpenRouter
  auth into a spawned Pi process

### What it must not claim to configure

It does **not** replace Pi's own independent provider login surface.

That distinction must be explicit in help text:

- WG-managed OpenRouter key: used by WG-native OpenRouter traffic and available
  for Pi env injection
- Pi-managed OpenRouter login: stored by Pi in `~/.pi/agent/auth.json`, used by
  Pi when running `pi:` routes on its own credentials

Proposed wording:

```text
`wg login openrouter` configures WG's OpenRouter key and endpoint.
Pi keeps its own login state separately. `pi:openrouter/...` routes may use
Pi-managed auth, WG-managed auth injected by `wg pi-handler`, or both.
```

## Design choices

### 1. New command, not a wrapper around `wg key set`

`wg key set openrouter` is too low-level and writes `api_key_env`/`api_key_file`
style config rather than expressing the preferred `api_key_ref` story. The new
UX should lead users directly to the modern secret-backed endpoint config and
avoid teaching deprecated migration paths.

### 2. Canonical endpoint name is `openrouter`

The check/status/help path becomes much simpler if the default login command
creates one canonical endpoint name for the common hosted OpenRouter case.

Advanced users can still create additional OpenRouter endpoints manually later.

### 3. Default to global config

Provider login is user-level state, like Pi's own global auth store. Defaulting
to global config avoids local projects silently failing because they do not
inherit local-only endpoint state.

Optional future flag:

```bash
wg login openrouter --local
```

The initial implementation does not need it.

### 4. Reuse WG secrets, do not add a second WG credential store

The goal is a simpler UX, not parallel secret plumbing. The login command
should be a front door over existing `wg secret`/endpoint primitives.

## Security notes

The implementation must keep the same or stronger credential hygiene than the
current low-level path:

- never accept API keys on argv in the recommended flow
- support `--from-stdin` for automation
- prompt with hidden input for interactive entry
- never print the key back to stdout/stderr
- never write the key into task logs, command examples, config diffs, or TUI
  summaries
- never persist plaintext `api_key = "..."` into config
- redact status output to presence/absence and secret-ref identity only
- if a shell snippet is printed, prefer stdin piping over `export ...` when the
  goal is WG-managed login

This aligns with existing `pi-handler` logging discipline, which already notes
that credentials are injected by environment and never by argv or logs. See
[src/commands/pi_handler.rs](/home/bot/wg/src/commands/pi_handler.rs:960).

## Implementation sketch

### CLI shape

Add:

```text
wg login <provider> [--check] [--from-stdin] [--set-default] [--global|--local]
```

Initial provider support:

- `openrouter`

Future providers can reuse the same command with provider-specific defaults and
secret-ref names.

### Suggested internal structure

New command module:

- `src/commands/login.rs`

Likely helpers:

- `run_login_openrouter(...)`
- `run_login_check_openrouter(...)`
- shared endpoint upsert helper that writes `api_key_ref`
- best-effort Pi-auth detection helper that checks for an `openrouter` entry in
  `~/.pi/agent/auth.json` without printing values

### Existing code areas likely involved

- CLI plumbing:
  [src/cli.rs](/home/bot/wg/src/cli.rs:2310),
  [src/main.rs](/home/bot/wg/src/main.rs:3480)
- endpoint config writing:
  [src/commands/endpoints.rs](/home/bot/wg/src/commands/endpoints.rs:245)
- secret storage and reachability:
  [src/commands/secret_cmd.rs](/home/bot/wg/src/commands/secret_cmd.rs:234),
  [src/secret.rs](/home/bot/wg/src/secret.rs:1)
- existing low-level key UX to avoid duplicating legacy behavior:
  [src/commands/key.rs](/home/bot/wg/src/commands/key.rs:1)
- config resolution / endpoint semantics:
  [src/config.rs](/home/bot/wg/src/config.rs:922)
- Pi/WG auth composition:
  [src/commands/pi_handler.rs](/home/bot/wg/src/commands/pi_handler.rs:100)
- docs/help text:
  [docs/GUIDE.md](/home/bot/wg/docs/GUIDE.md:390),
  [src/commands/quickstart.rs](/home/bot/wg/src/commands/quickstart.rs:534)

## Validation and test plan for the implementation task

Expected unit/integration coverage:

1. `wg login openrouter --from-stdin`
   - creates/updates canonical `openrouter` endpoint
   - writes `api_key_ref`, not `api_key_env` and not inline `api_key`
   - sets URL to `https://openrouter.ai/api/v1`

2. `wg login openrouter --check`
   - reports present/missing status without printing secret values
   - detects WG secret reachability correctly
   - reports Pi auth presence/absence without printing auth contents

3. existing manual config remains compatible
   - if user already has a named OpenRouter endpoint, login patches it
     predictably
   - if config already uses another endpoint as default, login does not steal
     default unless requested

4. Pi composition
   - `wg login openrouter` does not modify `~/.pi/agent/auth.json`
   - `pi:openrouter/...` still works with Pi-managed auth alone
   - `wg pi-handler` can still inject WG-managed auth when present

Suggested smoke coverage:

- `tests/smoke/scenarios/wg_login_openrouter.sh`
  - isolated `HOME`
  - `wg login openrouter --from-stdin`
  - assert config contains `api_key_ref`, canonical URL, and no plaintext key
  - `wg login openrouter --check` prints presence only, never the secret value
  - if seeded with a redacted fake Pi `auth.json`, check reports Pi auth present

- optional follow-up live smoke:
  - use a local OpenRouter-shaped endpoint and verify `wg login openrouter`
    composes with `wg nex -m openrouter:<model>` using the configured endpoint

## Recommended help/doc examples

Primary path:

```bash
wg login openrouter
wg login openrouter --check
wg model-scout
wg profile pi
```

Automation:

```bash
printf '%s' "$OPENROUTER_API_KEY" | wg login openrouter --from-stdin
wg login openrouter --check
```

Pi note:

```text
If you want `pi:` routes to use Pi's own stored login, run `/login` inside pi.
WG and Pi keep provider auth separately by design.
```
