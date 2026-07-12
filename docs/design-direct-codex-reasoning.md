# Direct Codex reasoning and Sol/Luna profile

WG's direct `codex:` handler keeps model identity, reasoning effort, and response
verbosity as separate settings.

## Activate

On a fresh installation, one command activates the built-in direct Codex profile:

```sh
wg profile use codex
```

It routes chat/default/standard/premium workers to
`codex:gpt-5.6-sol`, routes the weak assign/evaluate/FLIP roles to
`codex:gpt-5.6-luna`, and applies medium/high/xhigh/low effort by role or tier.
It uses the Codex CLI directly; Pi and the Pi plugin are not involved.

An existing `~/.wg/profiles/codex.toml` always wins over the built-in snapshot.
WG does not silently overwrite user-created profiles. To try the new built-in
while preserving an older customized file, rename that file first; to roll back,
restore it and run `wg profile use codex` again. To leave named-profile routing
entirely, run `wg profile use --clear` and restore or initialize the desired
base config.

## Reasoning adapter

Resolved WG reasoning becomes one Codex config override:

| WG | `model_reasoning_effort` |
|---|---|
| `off` | `none` |
| `minimal` | `low` |
| `low` | `low` |
| `medium` | `medium` |
| `high` | `high` |
| `xhigh` | `xhigh` |
| `max` | `max` |

These values were exercised with Codex CLI 0.144.1. Codex does not accept the
literal WG values `off` or `minimal`, so those translations are deliberate.
When WG reasoning is unset, WG emits no `model_reasoning_effort` override; the
user's `~/.codex/config.toml` remains authoritative. `model_verbosity` is never
inferred from reasoning and any existing verbosity configuration remains
independent. Pi routing is unchanged and continues to receive WG reasoning via
`--thinking`.
