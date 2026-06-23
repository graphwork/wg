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

### Formatting & lint MUST match CI (run `cargo fmt` before pushing)

CI's "Check & Lint" job (`.github/workflows/ci.yml`) fails fast on
`cargo fmt --check`, then runs `cargo clippy`, both on the **stable** toolchain.
rustfmt's output differs between stable and nightly (nightly collapses some
`assert!(...)` / method-chain forms that stable re-expands) and can drift across
stable releases — this caused two separate fmt-drift CI failures on the polish
branch.

The repo pins the toolchain in **`rust-toolchain.toml`** (`channel = "1.96.0"`,
with `rustfmt` + `clippy` components) so local and CI use the *same* rustfmt.
Because of that pin, the plain commands already do the right thing — **always run
these before committing/pushing**:

```
cargo fmt              # formats with the pinned stable rustfmt (matches CI)
cargo fmt --check      # must be clean — CI fast-fails here
cargo clippy           # same invocation CI runs
```

Do **not** format with `cargo +nightly fmt` or an editor configured to run a
nightly/standalone rustfmt — that reintroduces the drift. If `rustfmt`/`clippy`
aren't installed for the pinned toolchain, rustup auto-installs them on first
use; otherwise `rustup component add rustfmt clippy`. To bump Rust, edit
`channel` in `rust-toolchain.toml` (keep it `>=` the version CI's `@stable`
resolves to); the CI `nightly` job opts out via `cargo +nightly`.

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

#### Picking Pi models: `wg profile pi` (two-tier strong/weak)

The Pi profile splits its OpenRouter/Pi models into two stable tiers — **strong**
(chat + workers + heavy generative roles) and **weak** (the recoverable agency
one-shots: `.flip` / `.assign` / eval). `wg profile pi` is the "which model do you
want?" surface; it reads/writes `~/.wg/profiles/pi.toml` and, when `pi` is the
active profile, re-applies it as global config and hot-reloads so the next worker
picks up the new tier:

```
wg profile pi                       # show current strong/weak + routing (no-arg default)
wg profile pi --list                # list the models configured for the profile to pick from
wg profile pi <strong> <weak>       # set both positionally ('-' skips a tier)
wg profile pi --strong X --weak Y   # set both via flags (partial-update friendly)
wg profile pi --weak Y              # set only weak (the common scout case)
wg profile pi --strong X --dry-run  # preview; output is a copy-pasteable apply command
```

`strong`/`weak` are a 2-coloring of the existing three tiers (premium+standard →
strong, fast → weak) projected onto the normal `[tiers]` + `[models.<role>]`
keys — no new schema. Every set echoes the resulting assignment with `old → new`
so a transposed invocation is caught immediately. A lone positional is rejected
as ambiguous; explicit `[models.<role>]` overrides always win and are never
touched. See `docs/design-two-tier-pi-profile.md`.

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

### Agency one-shot tasks run on the weak tier

`.evaluate-*`, `.flip-*`, and `.assign-*` tasks are short one-shot LLM
calls (scoring + assignment verdicts), not full worker agents. They resolve
their model from the active profile's **weak** two-tier label (`tiers.fast`,
the cheap tier) via `resolve_agency_dispatch` — they do **not** follow the
project-level provider cascade from `coordinator.model` / `[models.default]`.
For the default (and `claude`) profile the weak tier *is* `claude:haiku` on
the claude CLI, so the historical pin is preserved and default behavior is
unchanged. A two-tier Pi profile that sets `--weak openrouter:deepseek/<model>`
now routes agency through DeepSeek automatically — no explicit per-role
overrides required.

Explicit `[models.evaluator]` / `[models.assigner]` / flip-role overrides in
config still win and keep their declared route (e.g. a `codex:` spec runs on
the codex CLI); the `coordinator.model` cascade is still ignored. This keeps
agency cheap while letting power users pin a specific provider per role.

Credential safety: agency verdicts are never *silently* dropped. When the
weak tier resolves to a keyless native-HTTP provider that needs an API key
(OpenRouter / OpenAI / anthropic-native, with no key in env or a matching
endpoint), it falls back **loudly** to `claude:haiku` on the claude CLI and
warns on stderr which key to set. An explicit per-role override is *not*
pre-empted at resolve time (explicit wins, keeps its route) but is still
protected at call time — `agency_native_lightweight_call` falls back to
`claude:haiku` on any native failure (invalid key, timeout, 5xx). claude /
codex CLI targets self-authenticate, so they are never downgraded.

The agent registry records each agency task under its resolved handler
(`executor=claude` for the default weak tier, the native / codex handler when
the weak tier or an override points there); the legacy `eval` / `assign`
labels are gone — they were always cosmetic. See the `resolve_agency_dispatch`
doc comment and `Config::weak_tier_spec()` in `src/service/llm.rs` /
`src/config.rs`.
Agency federation compatibility is exposed as `WG_AGENCY_COMPAT_VERSION = "1.2.4"` and import manifests record that compat surface for CSV/hash handshakes.
