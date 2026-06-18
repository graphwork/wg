<!-- wg-managed -->
# WG (project-specific guide)

This file is the **layer-2** project guide for agents working *on the
WG codebase itself*. It is NOT the universal chat-agent / worker-agent
contract — that is bundled inside the `wg` binary and emitted by:

```
wg agent-guide
```

Run `wg agent-guide` at session start (or read its output from a previous
session) to get the universal role contract: chat agent vs dispatcher vs worker
distinction, `## Validation` requirement, smoke-gate, cycle handling, git
hygiene, worktree isolation, "no built-in Task tool" rules, etc.

This file only covers things specific to the WG repo:

- How to use the `wg` command itself in this session
- How to develop and rebuild the `wg` binary
- Service configuration recipes (model / endpoint pairs)
- Named profiles (`wg profile use ...`) and secret backends (`wg secret ...`)
- Agency-task model pinning (a WG-only quirk)

For project orientation, run `wg quickstart`.

This guide is written to both `CLAUDE.md` and `AGENTS.md` and kept in
lock-step. The two files exist because Claude Code and Codex CLI look for
different filenames, but they should never drift in content. Any divergence is
a bug. Update both together.

---

## Use WG for task management

**At the start of each session, run `wg quickstart` in your terminal to orient yourself.**
Use `wg service start` to dispatch work — do not manually claim tasks.
Agents should run `wg` commands through bash/the terminal; there are no `wg_*`
tool calls.

## Development

The global `wg` and `nex` commands are installed via `cargo install`. After making changes to the code, run:

```
cargo install --path . --locked
```

to update both global binaries. This is the local `cargo install --path .`
install target, with `--locked` so Cargo uses the checked-in lockfile during
install. Forgetting this step is a common source of "why isn't this working"
issues when testing changes.

## Service Configuration

Pick a **(model, endpoint)** pair — the `wg` command derives the handler from the model spec's provider prefix:

- `wg config -m claude:opus` → claude CLI handler (no endpoint needed; CLI auths itself)
- `wg config -m codex:gpt-5.5` → codex CLI handler (no endpoint needed)
- `wg config -m nex:qwen3-coder -e http://127.0.0.1:8088` → in-process nex handler
- `wg config -m openrouter:anthropic/claude-opus-4-7` → in-process nex handler

The model prefix matches the handler / subcommand name (`claude` / `codex` / `nex`). The previous `local:` and `oai-compat:` prefixes for the in-process nex handler are deprecated aliases for `nex:`; they keep working for one release with a stderr warning, and `wg migrate config` rewrites them in existing config files.

The legacy `--executor` / `-x` flag and `[agent].executor` / `[dispatcher].executor` config keys are deprecated; they still work for one release with a deprecation warning, but the model spec is the single source of truth for which handler runs. Spawned agents continue to receive `WG_EXECUTOR_TYPE` and `WG_MODEL` env vars (handler kind + resolved model). See `src/dispatch/handler_for_model.rs` for the full mapping.

A fresh install with no `~/.wg/config.toml` already runs `claude:opus` via the
claude CLI handler — built-in defaults cover the common case. To commit choices
to disk run `wg config init --global` (minimal canonical claude-cli config; pass
`--route claude-cli` / `codex-cli` / `openrouter` / `local` / `nex-custom` for
non-default routes) or `wg setup` (interactive wizard). To inspect a config
without rewriting, run `wg config lint` (read-only companion to `wg migrate
config`). To clean up an old config with deprecated keys or stale model strings,
run `wg migrate config --dry-run` then `wg migrate config --all`. `wg config
-m/-e` auto-reloads the running daemon by default — pass `--no-reload` to skip.
See `docs/config-ux-design.md` for full details.

### Named profiles and secrets

Three starter profiles ship in the binary: `claude` (opus worker), `codex`
(gpt-5.5), `nex` (in-process endpoint). Activate one with `wg profile use
<name>`; use `wg profile use codex:gpt-5.5` or `wg profile use claude:opus`
to select a profile and pin the exact default/task-agent route in one step.
This writes `~/.wg/active-profile` and hot-reloads the daemon.
`wg profile show` / `list` / `create` / `edit` / `diff` / `init-starters`
cover the rest of the management surface. Profiles overlay onto the
global+local merge but never clobber project-local config.

#### Flipping the active profile and reverting (the round-trip)

The active profile is global state in `~/.wg/active-profile`. The chat agent
flips it to run a batch of tasks on Anthropic credits, then reverts to hand work
back to the in-process handler:

```
wg profile use claude     # flip: next workers run the claude profile (opus worker)
# ... dispatch / run a batch on Anthropic credits ...
wg profile use nex        # revert: back to the in-process localhost endpoint
```

`wg profile use codex` is the third flip target. Every `wg profile use` writes
`~/.wg/active-profile` and **hot-reloads the running daemon** — already-spawned
workers keep their model; the *next* worker the daemon spawns picks up the new
profile (no daemon restart). Pass `--no-reload` to stage the switch without
poking the daemon. Activation overlays on the global+local merge and never edits
project-local config.

#### `<name>:<route>` pins a model, it does not select an endpoint

The optional `:<suffix>` in `wg profile use <name>:<suffix>` is a **model spec**,
not an endpoint/route selector. It activates profile `<name>` and pins `<suffix>`
as the default + task-agent route in one step (`models.default`,
`models.task_agent`, `agent.model`, `dispatcher.model`, and the standard/premium
tiers all become `<name>:<suffix>`):

- `wg profile use claude:opus` → claude profile, default route pinned to `claude:opus`.
- `wg profile use codex:gpt-5.5` → codex profile, default route pinned to `codex:gpt-5.5`.

Because the suffix is a model id, **`nex:openrouter` is NOT the same as plain
`nex`, and does NOT route to OpenRouter.** Plain `wg profile use nex` uses the
profile's own default model (`nex:qwen3-coder-30b`) at the localhost endpoint
`http://127.0.0.1:8088`. `wg profile use nex:openrouter` instead pins the literal
model id `nex:openrouter` — i.e. it tells the in-process nex handler to send a
model named `openrouter` to that same localhost endpoint (the endpoint is
unchanged). That is almost never what you want.

To actually run through OpenRouter, use the `openrouter:` provider prefix (the
nex/native handler serves it), e.g. `wg config -m openrouter:anthropic/claude-opus-4-7`;
a bare `vendor/model` route launched on the nex handler with no endpoint is
auto-normalized to `openrouter:vendor/model`. There is no `wg profile use
openrouter:…` form — `openrouter` is a provider prefix, not a profile name, and
the model-qualified activation rejects it.

**Decision — nex's default route is left on localhost (unchanged).** We
deliberately do not repoint the `nex` profile's default to OpenRouter, because it
is not low-risk:

- The `nex` profile is by definition the in-process handler at a **localhost**
  endpoint (it mirrors the `wg nex` subcommand); repointing the default would
  change its identity and the local-endpoint contract the rest of these docs
  build on.
- The localhost endpoint needs no credential. An OpenRouter default would require
  an `OPENROUTER_API_KEY` and reintroduce the "openrouter configured but no key"
  silent failures the agency-pinning section below calls out.
- There is no single canonical OpenRouter model to adopt as the default.
- Even if set, `nex` and `nex:openrouter` would still differ: the suffix pins the
  bogus model id `openrouter`, not an `openrouter:`-prefixed route. The premise
  that the two could be made identical rests on reading the suffix as a route, so
  the fix is this documentation, not a config change.

API keys live in a credential store managed by `wg secret`. Endpoints
should reference keys via `api_key_ref = "keyring:<name>"` (preferred);
the older `api_key_env = "VAR_NAME"` is still accepted but
`wg migrate secrets` walks configs and rewrites them. Backends are
`keyring` (OS native, default), `keystore` (~/.wg/keystore, 0600), and
`plaintext` (requires `[secrets].allow_plaintext = true`). Passthrough URI
schemes (`op://...`, `pass:...`, `env:VAR`, `literal:...`) work without
storing the secret in `wg`.

### Agency tasks run on claude CLI

`.evaluate-*`, `.flip-*`, and `.assign-*` tasks are short one-shot LLM
calls (scoring + assignment verdicts), not full worker agents. They are
pinned to `claude:haiku` running on the claude CLI — the same handler
worker agents use — and ignore project-level provider cascade from
`coordinator.model`. This keeps agency cheap and immune to "openrouter
configured but no key" silent failures. Power users who *want* a
non-Anthropic provider for these roles can override per-role via
`[models.evaluator]` / `[models.assigner]` etc. in config; explicit
overrides win, cascade does not. The agent registry records these as
`executor=claude` (the legacy `eval` / `assign` labels are gone — they
were always cosmetic).
Agency federation compatibility is exposed as `WG_AGENCY_COMPAT_VERSION = "1.2.4"` and import manifests record that compat surface for CSV/hash handshakes.
