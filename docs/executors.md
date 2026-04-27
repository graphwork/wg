# Executors

An **executor** is the runtime wg uses to drive an LLM for a spawned agent.
Different executors wrap different upstream tools, talk to different APIs, and
have different reliability and feature trade-offs. You pick one with `-x` on
`wg init`, with `--executor` on `wg config`, or implicitly via a named route
on `wg setup --route <name>`.

## Selection guide

| Executor | Wraps | Endpoint / auth | When to use it | Status |
|---|---|---|---|---|
| `claude` | `claude` CLI binary | Anthropic; auth handled by `claude` CLI login | Anthropic models (Opus / Sonnet / Haiku); your default if you have a `claude` CLI session | Stable — recommended default |
| `codex` | `codex exec` CLI binary | OpenAI by default; any OAI-compat endpoint via `-e <url>` | OpenAI models, **and** any OAI-compatible server (local llama.cpp / vLLM / Ollama / Lambda Labs / OpenRouter) | Stable — recommended for OAI-compat |
| `nex` (a.k.a. `native`) | wg's in-process OAI-compatible HTTP client | OAI-compat endpoint, configured via `-e <url>` | Historically: bring-your-own OAI endpoint when no CLI binary was viable | **Fragile — being phased out.** Re-implements the agent loop in-process; faults after the first turn under some configs. Switch to `codex` if your endpoint is OAI-compat. See the migration note below. |
| `shell` | A shell command (`task.exec`) | None (no LLM) | Pure scripted work — lints, file moves, build kicks. Carries no model or endpoint. | Stable |
| `amplifier` | The Amplifier multi-agent wrapper | Auth handled via amplifier settings | Multi-agent delegation + bundled context | Stable for amplifier users |

Pick by **what kind of model and what kind of auth**, not by performance —
all of these spawn a per-task subprocess, so steady-state perf is dominated
by the model itself.

## `claude`

The default. Pty-wraps the `claude` CLI binary. Auth, retries, tool-use,
streaming, prompt caching, and history are all handled by the `claude` CLI
outside wg, which is exactly why it's the most reliable path. No endpoint
flag — `claude` talks to Anthropic.

```bash
wg init -x claude
wg setup --route claude-cli --yes        # equivalent route-driven setup
```

## `codex`

Pty-wraps the `codex exec` CLI binary (one invocation per inbox turn,
clean process boundary between turns — same architectural shape as the
`claude` executor, which is why it inherits the same reliability story).
Talks to OpenAI by default, **or** any OAI-compatible endpoint when you
pass `-e <url>` to `wg init`.

### Custom OAI-compat endpoint walkthrough

This is the user-facing recipe for a custom OAI-compatible endpoint
(`lambda01`-style local cluster, vLLM, Ollama, llama.cpp, an OpenRouter
proxy, etc.). Drop-in:

```bash
# 1. Make sure the codex CLI is installed and on $PATH.
#    Single Rust binary; release downloads at https://github.com/openai/codex
codex --version

# 2. Export your endpoint's API key (skip if the endpoint takes any value).
export OPENAI_API_KEY=sk-...

# 3. Initialize the workgraph with the codex executor + your endpoint + model.
wg init -m qwen3-coder \
        -e https://lambda01.tail334fe6.ts.net:30000 \
        -x codex

# 4. Boot the dispatcher and start chatting.
wg service start
wg tui
```

That's the entire flow. You do **not** need to hand-edit
`~/.codex/config.toml` — wg passes per-invocation `--config` overrides to
`codex exec` (model provider name, `base_url`, `env_key`, and
`wire_api = "responses"`) so the redirection is scoped to the spawned
subprocess and your own codex config is untouched. See
[`src/commands/codex_oai_compat.rs`](../src/commands/codex_oai_compat.rs)
for the exact override list.

### Env vars wg honors

- `WG_ENDPOINT_URL` (set automatically by `wg init -e <url>`) —
  redirects codex's base URL away from `api.openai.com`.
- `WG_API_KEY` or `OPENAI_API_KEY` — the value codex sends as the
  bearer token. wg checks `WG_API_KEY` first, then falls back to
  `OPENAI_API_KEY`. If neither is set, codex falls back to its own auth
  (which only works against `api.openai.com`).
- `OPENAI_BASE_URL` — codex's own override; honored if `WG_ENDPOINT_URL`
  is unset.

### Wire-format note

wg pins `wire_api = "responses"` because codex 0.120+ removed the
`"chat"` wire format. Endpoints that only implement OAI Chat Completions
(and not the Responses API) will reject codex's POST to
`<base_url>/responses` — that's a wrapper-target gap, not a wg gap.
Most modern OAI-compat servers (vLLM ≥ 0.5, Ollama ≥ 0.4, recent
llama.cpp, OpenRouter) speak Responses; older deployments may need to
upgrade.

## `nex` (a.k.a. `native`)

In-process OAI-compatible HTTP client. Re-implements the agent loop
(auth, retries, streaming, history, tool-use) inside the wg binary
rather than wrapping a CLI tool. Routes accepted by `wg setup`:
`openrouter`, `local`, `nex-custom`.

### Why it's being phased out

Re-implementing a mature CLI's agent loop is a large surface, and the
in-process loop has a known fault that hits after the first message in
some configs (see
[`docs/research/thin-wrapper-executors-2026-04.md`](research/thin-wrapper-executors-2026-04.md)).
The codex thin-wrapper path now covers every OAI-compat workload that
`nex` was carrying, with a per-turn process boundary that is
architecturally more reliable. New projects should pick `codex` over
`nex` for any OAI-compat endpoint.

### Migration: `nex` → `codex`

Switching is a one-flag change. Same model, same endpoint, same key:

```bash
# Before
wg init -m qwen3-coder -e https://lambda01.tail334fe6.ts.net:30000 -x nex

# After
wg init -m qwen3-coder -e https://lambda01.tail334fe6.ts.net:30000 -x codex
```

For an existing project, edit `.workgraph/config.toml` and change
`executor = "nex"` (or `"native"`) to `executor = "codex"` under both
`[agent]` and `[coordinator]` (a.k.a. `[dispatcher]`), then
`wg service reload`. The endpoint URL and model carry over unchanged —
codex picks them up via `WG_ENDPOINT_URL` and the same API-key env vars
described above.

If your endpoint only speaks OAI Chat Completions and not the Responses
API, codex won't work for you yet — keep `nex` until the endpoint
upgrades, and treat that as a known limitation rather than a wg bug.

## `shell`

Runs a literal shell command from `task.exec`. No model, no endpoint —
the agent isn't an LLM, it's the shell. Use for deterministic, scripted
steps inside a graph that otherwise contains LLM tasks.

```bash
wg add "Run formatter" --exec-mode shell --exec "cargo fmt --all"
```

## `amplifier`

Wrapper for the Amplifier multi-agent system; provides bundled context
and delegation patterns Amplifier-native users expect. Auth is handled
by amplifier itself.

```bash
wg config --executor amplifier
```

## Further reading

- Research and rationale for the codex-as-thin-wrapper choice:
  [`docs/research/thin-wrapper-executors-2026-04.md`](research/thin-wrapper-executors-2026-04.md)
- Per-route setup walk-throughs:
  [`docs/guides/openrouter-setup.md`](guides/openrouter-setup.md)
- Service / dispatcher configuration: [README — Service](../README.md#service)
