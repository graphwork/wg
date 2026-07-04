# Manual Smoke Notes: `setup-provider-login-onboarding`

Date: 2026-07-04

## First-run command sequences

OpenRouter, repo-local, store key in WG:

```bash
cd /path/to/repo
printf '%s' "$OPENROUTER_API_KEY" | wg setup --route openrouter --scope local --from-stdin --backend keystore --yes
```

Expected verification commands:

```bash
wg login openrouter --check
wg models fetch --no-cache
wg status
```

Expected outcome:

- `.wg/config.toml` contains `api_key_ref = "keystore:openrouter"` or another secret ref
- `.wg/config.toml` does not contain the raw key and does not contain `api_key = "..."`.

Pi, repo-local, reuse existing global WG OpenRouter login for native WG traffic:

```bash
wg login openrouter --check
cd /path/to/repo
wg setup --route pi --scope local --yes
```

Expected verification commands:

```bash
wg profile pi --show
wg login openrouter --check
wg status
```

Expected outcome:

- `.wg/config.toml` keeps the strong worker/chat route on `pi:openrouter/...`
- repo-local config reuses the global WG OpenRouter login via `inherit_global = true`
- repo-local config does not copy the global secret value.

Claude CLI, self-auth, no API key prompt:

```bash
cd /path/to/repo
wg setup --route claude-cli --scope local --yes
```

Expected verification commands:

```bash
wg status
wg setup --help
```

Expected outcome:

- no OpenRouter/API-key prompt during setup
- repo config points at `claude:...` models and uses the self-auth CLI path.
