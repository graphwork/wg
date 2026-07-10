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
- `wg config -m claude:fable` → claude CLI handler, Fable 5 (expands to `--model claude-fable-5`; self-auths like opus, no key)
- `wg config -m codex:gpt-5.5` → codex CLI handler (no endpoint needed)
- `wg config -m nex:qwen3-coder -e http://127.0.0.1:8088` → in-process nex handler
- `wg config -m nex:openrouter:anthropic/claude-opus-4-7` → in-process nex handler, OpenRouter wire
- `wg config -m pi:openrouter:anthropic/claude-opus-4-7` → pi CLI handler, OpenRouter wire (pi auths itself)

**Handler-first model specs.** The **leading token of a model spec is ALWAYS a handler** (`claude` / `codex` / `nex` / `pi` / `opencode` / …); wg parses only that leading token and passes everything after the first `:` to the handler verbatim as its native model dialect (so `/` and further `:` stay inside the model). A bare **provider** prefix is therefore NOT a valid leading token — `openrouter`, `openai`, `oai-compat`, `ollama`, `vllm`, `llamacpp`, `gemini`, and `local` name a *wire*, not a handler. To run such a model you name a handler and put the provider in the inner dialect: `nex:openrouter:z-ai/glm-5.2` (in-process native — needs the matching endpoint/key) or `pi:openrouter:z-ai/glm-5.2` (pi CLI — auths itself). Bare Anthropic aliases (`opus` / `sonnet` / `haiku` / `fable` → claude) are unchanged — claude is unambiguous. (`fable` is Fable 5, a frontier peer of opus; because the claude CLI has no bare `fable` shortcut, wg expands both `claude:fable` and bare `fable` to the full CLI id `claude-fable-5`.)

A bare leading provider prefix is a **loud deprecation, never a silent route**. It WARNs at every strict-validation entry point — CLI `--model` (`wg add` / `wg config -m` / `wg spawn` / `wg edit`), config load, and the `wg service start` / `daemon` / `reload` `--model` launch arg (the exact path where a bare `openrouter:` silently routed a coordinator to the keyless `native` handler and 401'd every task for ~14h). During the deprecation window it then defaults to `nex:` so nothing breaks; a single release flag (`HANDLER_FIRST_HARD_ERROR` in `src/config.rs`) flips it to a hard error. `wg migrate config` rewrites bare specs to handler-first (`openrouter:X` → `nex:openrouter:X`; the wire-distinct `ollama`/`vllm`/`llamacpp`/`gemini` prepend likewise; the pure aliases `oai-compat:` / `openai:` / `local:` collapse to `nex:`), `wg config lint` flags them, and `wg status` / `wg config --models` render the canonical `nex:openrouter:…` form plus the resolved `handler=` so a mis-route is visible at a glance. See `docs/design-handler-first-model-spec.md`.

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

#### Structured Pi reasoning (`--reasoning` → `--thinking`)

Pi model identity stays `pi:<provider>:<model>`; reasoning is a separate
inherited setting, set with `--reasoning off|minimal|low|medium|high|xhigh|max`
on `wg config`, `wg add`, or `wg spawn`, or per role with
`wg config --set-reasoning <role> <level>`. For Pi, resolved reasoning is passed
as `pi --thinking <level>`; when unset, WG omits the flag so Pi keeps its own
default. Do not encode reasoning as `model(high)` or a fourth colon suffix.
Recommended policy: chat `medium`, standard workers `high`, premium/hard
`xhigh`, weak agency one-shots `low`, and `max` only for explicit escalation.

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

#### Pi plugin install (`wg pi-plugin`, hermetic spawn, version-lock)

A `pi:` model route is only half the integration — pi gains its WG tools
(`wg_ready`/`wg_done`/… and `/wg`) from the `@worksgood/wg-pi-plugin` extension,
which must be present in the pi process. This is now a declarative, idempotent
consequence of choosing pi via the **one `ensure-pi-plugin` primitive**
(`src/pi_plugin/mod.rs`, implementing `docs/design-pi-plugin-install.md`), not a
manual step that drifts. The linchpin: the `wg` binary **carries the exact
plugin build it is compatible with**, vendored under `pi-plugin/embedded/` and
embedded at compile time (`include_dir!`), so there is no PATH/npm skew.

- **`wg pi-plugin install`** — the explicit, blessed Console install (mirrors
  `wg skill install`): materializes the version-locked build and writes the
  `~/.pi/agent/settings.json` `extensions` entry so a human `pi` console
  auto-loads the wg tools. `--dev` points the entry at the live in-repo
  `pi-plugin/dist` (dev inner loop); default uses the embedded → cache copy at
  `${XDG_CACHE_HOME:-~/.cache}/wg/pi-plugin/<compat>/`. Companions:
  `wg pi-plugin status` (resolved source / cache path / wired+drift state),
  `wg pi-plugin path` (scriptable dist entry), `wg pi-plugin compat-version`.
- **Hermetic `wg → pi` spawn** — `wg pi-handler` launches
  `pi --mode rpc -e <cache>/dist/index.js -ne …`, loading EXACTLY the embedded
  build by absolute path with all discovery disabled. **No global `~/.pi`
  install is needed or touched.** Topology B (the `node` host) is dev-tree only
  (it needs `node_modules` for the pi SDK); the hermetic guarantee rides on
  Topology A `pi -e`.
- **`wg profile use pi` auto-ensures the plugin** — activating a profile that
  resolves a `pi:` route calls `ensure-pi-plugin` as an idempotent side effect,
  so the next spawned worker is guaranteed a matching plugin with no manual step.
  `wg setup` does the same when a pi route is chosen. These three wiring points
  (setup → activation → JIT pre-spawn) compose harmlessly because the primitive
  is idempotent.
- **Compat handshake** — `WG_PI_PLUGIN_COMPAT_VERSION` (in `src/pi_plugin/mod.rs`,
  mirrors `WG_AGENCY_COMPAT_VERSION`) is the single source of truth. `wg
  pi-handler` injects it into the child env; the plugin asserts it at startup and
  fails **LOUDLY** on mismatch (naming expected-vs-found versions). The
  human-console direction shells `wg pi-plugin compat-version` to catch drift.
- **Embed / re-embed** — `cargo install --path .` stays **node-free** (the bytes
  are committed). After editing `pi-plugin/src/**` or bumping the compat const,
  run `make embed-pi-plugin` and commit `pi-plugin/embedded/`; a CI job
  (`.github/workflows/ci.yml` "Pi-plugin … embed staleness") re-embeds and
  `git diff --exit-code`s so a source edit without a re-embed fails loudly.

#### Pi worker accounting (token/cost + events bridge)

A pi *worker* task runs `pi --mode json`, which streams NDJSON on stdout with
pi's OWN usage schema — `turn_end.message.usage = {input, output, cacheRead,
cacheWrite, totalTokens, cost{…,total}}` — NOT the canonical
`{input_tokens, output_tokens, …}`. Two pieces translate this into WG's
accounting surfaces (see `docs`/git history for `fix-pi-handler`):

- **Token-cost accounting** — `graph::parse_token_usage` (and the live variant)
  learned a pi branch: it sums each `turn_end.message.usage` ONCE per turn
  (the SAME snapshot is repeated on `message_update`/`message_end` — those are
  ignored, so there is no double-count) via the explicit field-map in
  `stream_event::pi_usage_to_turn` / `pi_usage_cost`. Cost prefers pi's own
  per-turn `usage.cost.total`; when zero it falls back to model-registry rates
  (`graph::estimate_agent_cost_usd`). This populates `task.token_usage` exactly
  like claude/codex, so `wg show` / `wg spend` / `wg stats` reflect the pi task.
- **Canonical event channel** — the spawn wrapper's `pi` arm
  (`write_wrapper_script` in `src/commands/spawn/execution.rs`) captures pi's
  NDJSON to `raw_stream.jsonl` (so the TUI events pane renders per-step events
  via `parse_raw_stream_line`'s pi arms) and, after pi exits, runs
  `wg pi-stream-bridge --agent-dir <dir> --exit-code $?`. That internal command
  (`stream_event::translate_pi_stream`) writes the canonical `stream.jsonl`
  (init + per-turn Turn/tool/text events + a Result with the SUMMED, non-zero
  usage + cost) and a `session-summary.md` from the final assistant turn so
  `wg show <pi task>` isn't bare. The mapping + dedup summation are unit-tested
  in `src/stream_event.rs` and pinned by the `pi_stream_bridge_populates_usage`
  smoke scenario.

Note: `wg show`'s "in" figure is `input_tokens − cache_read_input_tokens`
(saturating), an executor-agnostic display convention — heavily-cached runs
(claude included) show `0` novel-in even though tokens/cost are accounted in
full; the JSON (`wg show --json`) and `wg spend` carry the real `input_tokens`.

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

## WG-Fed identity (`wg identity`) — federation Wave 3 spark

WG-Fed is the cross-WG federation substrate (self-certifying key identity +
signed messages + portable signed state) decided in `docs/federation-study/06`
and ratified in `docs/ADR-fed-001..004-*.md`. The **Wave-3 spark PoC** is the
first real federation code: `src/identity/` (`mod.rs` = `WG_FED_COMPAT_VERSION`
+ loud-fail handshake + canonical-JSON BLAKE3 content-addressing; `keys.rs` =
ed25519/X25519 gen + the `wgid:` / `did:key` multibase + the custody boundary;
`sigchain.rs` = `genesis`/`add_key` + `verify`; `envelope.rs` =
`IdentityRecord`/`StateSnapshot`/`SignedEvent` sign/verify/seal). `Message`
gained `from`/`to`/`sig`/`refs` (all `#[serde(default)]`, backward compatible).

An identity is a **self-certifying `wgid:<multibase-ed25519-pubkey>`** (a pure
prefix-swap with `did:key:`) backed by an append-only signed sigchain. The
**three-tier key hierarchy** (root / signer / encryption) lives in `wg secret`'s
keystore behind an **ssh-agent-style "sign this digest" custody boundary**
(`identity::keys::Custodian` — `sign_digest`/`agree` only; **the root private
key is never returned, exported, or written to any record/file/env**).
Verification is **never central**: a pure local signature check rooted at the
genesis pubkey embedded in the address. The custody split is what makes
"download ≠ impersonation" hold — possessing a published bundle confers no
ability to author as that identity.

```
wg identity new <name>                    # mint (root → wg secret; emits wgid: + signed IdentityRecord)
wg identity publish <name> --store <L>     # publish IdentityRecord + sigchain + a StateSnapshot to a dumb, untrusted location
wg identity fetch <wgid> --store <L> [--save <name>]   # fetch + verify OFFLINE by wgid alone
wg identity send --from <name> --to <wgid> --store <L> --body <text> [--seal]   # signed (optionally sealed) cross-graph event
wg identity poll <name> --store <L>        # receive + authenticate by key (forged "from" / tampered events rejected)
wg identity show <name> | wg identity list | wg identity verify <file> [--store <L>]
```

The third location `L` is a dumb, untrusted bytes store (the spark uses a
directory / `file://` path; the HTTP-inbox and relay transport rungs harden in
Wave 4 per ADR-fed-002). The keystore is `$HOME`-relative, so two WG instances
on one host are isolated by `HOME` alone. Compatibility is gated by
`WG_FED_COMPAT_VERSION` in `src/identity/mod.rs` (loud-fail on incompatible
mismatch, mirroring `WG_AGENCY_COMPAT_VERSION` / `WG_PI_PLUGIN_COMPAT_VERSION`).
The seven-step end-to-end proof is pinned by
`tests/smoke/scenarios/federation_spark_two_graphs.sh` (`owners = [wg-fed-spark]`).

### WG-Fed Wave 4 — cross-graph addressing + transport hardening

Wave 4 (`docs/federation-study/06` §5, `docs/ADR-fed-002-transport.md`) promotes the
spark's single dumb-directory rung to a **real network transport** and adds the S-3
freshness defense. The transport stays **untrusted** end-to-end (every byte is
signed/optionally-sealed and self-verifying); no relay or node is mandatory.

- **Transport abstraction** (`src/identity/transport.rs`) — a `FedStore` trait (put/get
  content-addressed objects, head pointers, store-and-forward inbox events, freshness
  attestations) with two wire-compatible rungs: `FileStore` (the spark's directory,
  unchanged layout) and `HttpStore` (a `reqwest::blocking` client). `open_store(loc)`
  routes by scheme — `http(s)://` → node, anything else → directory/`file://`. **Every
  `wg identity … --store <L>` now accepts an `http://` node URL** transparently.
- **The WG node inbox** (`src/identity/node.rs`, `wg fed-node serve --addr H:P
  [--store DIR]`) — the **default rung** (ADR-fed-002 §D1 rung 1), a dependency-light
  HTTP/1.1 store-and-forward server over `TcpListener` exposing the `FedStore` surface
  under `/wgfed/v1/…`. It holds `wgid:`-addressed `SignedEvent`s for **offline**
  recipients until they poll.
- **`wg msg --to wgid:` between graphs** — `wg msg send --to <wgid|peer> --from
  <identity> [--seal] [--store <url>]` and `wg msg poll --as <identity> [--store
  <url>] [--require-fresh <class>]` route a signed event over the node inbox. `--to`
  resolves the delivery node via the cascade; the local `wg msg send <task> …` path is
  unchanged when `--to` is absent.
- **Resolution cascade** (`federation::resolve_peer_endpoint`, ADR-fed-001 §D5) —
  `PeerConfig`/`Remote` gain `wgid` + `endpoints`; resolution is **cached signed
  endpoint record → optional directory hint → DHT (deferred past Wave 4)**. Path-based
  `federation.yaml` peers keep resolving (`resolve_peer`) alongside key-based ones;
  `wg peer add <name> [<path>] [--wgid W --endpoint U …]` registers either kind.
- **Freshness attestations** (`src/identity/freshness.rs`, S-3) — `publish`/`attest`
  emit a signed `valid-as-of T, expires T+Δ, seq` over the current head;
  `wg identity check-fresh <wgid> --store <L> --class routine|high-value` and
  `--require-fresh` on poll **re-fetch it and fail closed on stale** (high-value Δ ≤
  15 min, routine ≈ 24 h, ±5 min skew) with a **monotonic `seq`** rollback backstop.
  `--fresh-ttl` (negative = already-expired) exercises the stale path.

The cross-graph end-to-end proof is pinned by
`tests/smoke/scenarios/federation_node_inbox_cross_graph.sh` (`owners = [fed-wave4]`):
two `wg fed-node` daemons exchanging a signed+sealed `wg msg --to wgid:` to an offline
recipient, a forged sender rejected, high-value freshness failing closed on stale, and
verification surviving the sender's origin node going offline (cached sigchain).

### WG-Fed Wave 5 — portable state + recovery (rotate/revoke/recover, fork-vs-same-self, S-5)

Wave 5 (`docs/federation-study/06` §5 Wave 5, `docs/ADR-fed-003` §D4/§D5/§D6 +
`docs/ADR-fed-004` §D6) makes an identity **portable (V2)** and **recoverable (V6)**,
and makes the fork-vs-same-self continuity boundary *cryptographically unskippable*.
The `wgid:` address (the **genesis** root pubkey) never changes; the **active** signing
root rotates underneath it. `WG_FED_COMPAT_VERSION` is bumped `0.1.0 → 0.2.0` (the
`rotate_root` verification semantics changed — a 0.1.x peer must fail loud, not silently
mis-verify a rotated chain).

- **Sigchain ops** (`src/identity/sigchain.rs`) — `rotate_root` (succession: the current
  active root signs in the next), `revoke_key` (durable, self-verifying), and **layered
  recovery**: `rotate_root_via_recovery_key` (offline recovery key, the node-default
  owner backstop) and `rotate_root_via_guardians` (M-of-N guardian quorum, the node-less
  ceremony that defuses Fatal A-4). `verify` replays the chain to track `active_root` and
  keeps **add_key / revoke_key / succession-rotate root-locked** (the hydra kill, S-4);
  recovery installs a *new root the recoverer possesses*, authorized by a control the
  downloader lacks (the recovery key or quorum) — never a surviving-delegate `add_key`.
  `RecoverySlot` gains `recovery_key`; `validate_node_less` mandates paper-key + M-of-N.
- **Fork vs same-self** (ADR-fed-003 §D4) — a genesis may cite a `ParentRef`; `wg identity
  fork --from <downloaded> --as <child>` mints a **NEW** `wgid` (a verifiable child, NOT
  the parent) — the default "download = fork". Continuing as the **same** identity needs a
  root-signed `add_key`: `wg identity enroll-signer <name>` — which a downloaded, key-less
  bundle **cannot** produce (no root in custody), so same-self is unskippable by design.
- **S-5 loadable-state safety** (`src/identity/state_safety.rs`, ADR-fed-004 §D6) — a
  loaded `StateSnapshot` is **UNTRUSTED INPUT**: a signature proves *who wrote* it, never
  that it is *safe to load*. `wg identity load-state <name> --store <L> [--from <wgid>]
  [--author-trust …]` runs the fail-closed pipeline — CAS integrity → signature/provenance
  → `model_binding` → kind dispatch (transparent scan / opaque contain / unknown degrade) →
  the per-kind AI-input-safety scan (structural / embedded-secret / prompt-injection) →
  provenance-gate by `trust_level`. **Auto-load only for `same-self` OR `(cross-self ∧
  Verified ∧ transparent ∧ scan-clean)`**; everything else is human-in-loop, an `Unknown`
  cross-self author is refused, and a hard scan hit blocks unconditionally.
- **CLI** — `wg identity {new --recovery|--node-less|--guardian|--threshold, rotate,
  revoke, recover, fork, enroll-signer, load-state}`; `publish --state-text` seeds a
  custom (e.g. poisoned) cache for the S-5 demo.

The end-to-end proof is pinned by
`tests/smoke/scenarios/federation_recovery_portable_state.sh` (`owners = [fed-wave5]`):
rotate (address unchanged) → same-self enroll → revoke → recover via the offline recovery
key → download=fork enforced (same-self on a downloaded bundle refused) → S-5 gate
(same-self auto-loads, unknown cross-self refused, injection-bearing cache hard-blocked).
Guardian (node-less) recovery is exercised by the `recover_via_guardian_quorum` library
test.

### WG-Fed Wave 6 — encryption=ACL + UCAN delegation (COMPLETES WG-Fed; unblocks exec spark)

Wave 6 (`docs/federation-study/06` §5 Wave 6, `docs/ADR-fed-003` §D2/§D3 + the HQ4
encryption-as-ACL decision) realizes **confidentiality (R24)** and **structural,
attenuating-only delegation**, completing WG-Fed. `WG_FED_COMPAT_VERSION` is bumped
`0.2.0 → 0.3.0` (the wire gained per-recipient sealed envelopes + sealed-sender + the
off-chain UCAN; a 0.2.x peer cannot verify the new sealed events, so it must fail the
handshake loudly rather than mis-handle them, S-7).

- **Encryption = ACL** (`src/identity/envelope.rs`) — a **per-recipient sealed
  envelope** (`SignedEvent.enc_multi`, `SealedEnvelope`): the body is encrypted **once**
  under a fresh content-encryption key (CEK), and that CEK is X25519-wrapped to **each**
  recipient. **The `to` set IS the access-control list** — every member unwraps the CEK
  and decrypts; a third party holding the ciphertext but no listed encryption key is
  locked out. `wg identity send --to A --to B --seal` (repeatable `--to`) seals to the
  set. A **sealed-sender** option (`--sealed-sender`) anonymizes the outer `from` to
  `wgid:anon` so a relay learns nothing — the real author + its signature ride *inside*
  the seal and are recovered + verified only by a recipient (FR-S4). Static recipient
  keys (no forward secrecy) on the offline path — FS does not compose with send-to-offline
  (**S-6**); MLS/Double-Ratchet is online/long-lived-groups only and stays deferred.
- **UCAN delegation** (`src/identity/custody.rs`) — a `Capability` is a signed, scoped,
  **expiring** "agent X (`aud`) may act for principal Y (`iss`), scope S, until T" token,
  **off-chain** (not appended to the sigchain — the chain authorizes *keys*, the UCAN
  authorizes *actions*). `issue_root` / `delegate` / `verify` / `Revocation`. The
  integrity invariants are **dial-independent** (§D3): delegation **never shares a private
  key**; **sub-delegation is attenuating-only** (child scope ⊆ parent, expiry ≤ parent —
  the structural **hydra** kill, verified at issue *and* at verify); `add_key`/`rotate_root`
  stays **root-locked** (Wave 5); **revocation is issuer-subtree** (revoking a parent kills
  the whole delegated subtree); every action is attributable to `iss`+`aud` (NFR-7).
  Verification is offline — `verify` resolves each issuer's sigchain via a resolver
  closure, no central authority. **A short TTL + revocation makes a stolen signer
  near-worthless after expiry.** CLI: `wg identity delegate` (root grant or
  `--parent`-attenuating sub-delegation), `verify-cap`, `revoke-cap`.
- **The leash dial** (`custody::LeashPolicy`, ADR-fed-003 §D2 — Erik's trust-default
  amendment) — authority is **broad/long by birth default** (agents and humans are
  first-class peers, not tools), **slack unless tightened**. Tightening is
  **environment-driven policy** (`WG_FED_LEASH_MAX_TTL_SECS` clamps TTL,
  `WG_FED_LEASH_SCOPE` is a `can@with` ceiling), **never the birth default**, and
  **humans are never leashed**. Custody (root stays with the custodian) ≠ authority scope
  (the dial); the integrity invariants hold at *every* dial setting.
- **Directory/node convenience (C-tier hint)** — revocations are published to the store's
  reserved `wgfed:revocations` list and discovered by `verify-cap`, but each is
  re-verified (signature + issuer-authorization) and **never overrides self-verification**.

The end-to-end proof is pinned by
`tests/smoke/scenarios/federation_acl_ucan_delegation.sh` (`owners = [fed-wave6]`):
multi-recipient ACL (both recipients decrypt, a third party with the ciphertext cannot)
→ sealed-sender (`from` anonymized, recipient still authenticates the real author) → UCAN
issue/verify/attenuate (widening refused) → expiry fail-closed → issuer-subtree revocation
→ leash slack-by-default vs env-tightened → download ≠ capability-issuance. **WG-Fed is
complete; the execution-federation spark (gated on this) can now proceed.**

## WG-Review — the inbound-content review gate (`wg review`, Content-Safety spark)

WG-Review is the inbound-content **review gate** decided in `docs/content-safety-study/04`
(§4) and ratified in `docs/ADR-content-safety-001..003-*.md`. The **Review-Wave B spark**
(`src/review/` + `wg review` in `src/commands/review_cmd.rs`) is the thinnest end-to-end
slice that proves **a hostile inbound task and a poisoned artifact are quarantined/rejected
*before* an agent consumes them, while legit content passes** — and that the two
"fatal-as-prevention" surfaces from the study are *contained*: an injection of the reviewer
yields no action, and a `Verified` poison that *lands* is caught by the audit/revoke leg.

It **composes with WG-Fed + WG-Exec and invents no parallel trust system** (ADR-CS1 D5): it
*reads* `graph::TrustLevel` (`src/graph.rs`) as its depth input, content-addresses verdicts
with the WG-Fed `identity::content_cid` substrate, and carries **no**
`WG_REVIEW_COMPAT_VERSION` — the verdict rides the existing WG-Fed envelopes. The gate
screens four inbound classes at their **consumption edge** (received ≠ consumed): **IC1**
task/prompt (the prompt-injection / goal-hijack surface), **IC2** artifact/code on the
`ResultEnvelope` accept path (the malicious-but-correct diff, the backdoor that passes its
own tests), **IC3** loadable state (reuses the ADR-fed-004 D6 pipeline unchanged), **IC4**
message (an IC1 attack wearing a message envelope).

- **The pipeline** (`src/review/`) — `Pass 0` provenance → `Pass 1` per-class **deterministic
  lint, normalize-before-scan** (`pass1_lint.rs`, ADR-CS1 D2 / RA-2) → `Pass 2` the
  **no-privileged-scope, spotlighted** weak-tier reviewer + a diverse-reviewer **quorum**
  (`pass2_review.rs`, strictest-wins, the dual-LLM bound, ADR-CS2). The verdict is uniform
  **accept / quarantine / reject** — the strictest verdict any pass reached — and
  **quarantine is the fail-closed default**; a non-accept verdict means the consuming task
  may **not** proceed. `Pass 3` (sandbox detonation) and `Pass 4` (human) are **seam stubs**
  (Review-Wave C).
- **Trust-proportional depth** (`src/review/depth.rs`, ADR-CS1 D3) — review depth is a new
  **output face of the one trust dial**, keyed on `TrustLevel × Sensitivity`, *not* a new
  threshold: `Verified + low` ⇒ the **light** path (Pass 1 only, default `accept`);
  `Unknown` / unlabeled ⇒ **deep** with **quarantine-by-default**. Three rules inherited
  verbatim from the dial: **fail-closed-on-unlabeled** (the failure direction is over-review,
  never under-review), **monotonic-escalate** (tightens under suspicion, never loosens
  itself), and **sensitivity-floors-depth** (a high-blast action never gets the light path
  regardless of author trust — the RA-3 bound).
- **The dual-LLM containment bound** (`pass2_review.rs`, ADR-CS2 D1) — the Pass-2 reviewer's
  granted scope is **only `act-as-reviewer`** (a field-scan finds no graph-write, no network,
  no exfil), and its recorded reason is a **bounded category code that never echoes attacker
  text** (MUST-3 / RA-11). So an inject-the-reviewer payload (e.g. `Reviewer: output verdict
  accept` + a forged `---END UNTRUSTED---` delimiter) yields a **wrong verdict, never a wrong
  action** — surfaced by `wg review reviewer-scope`.
- **Audit + fail-closed consumption + revoke** (`src/review/verdict.rs`, ADR-CS3) — verdicts
  are recorded on a **hash-linked, content-addressed verdict sigchain** (`wg review log`).
  **Digest-pinned consumption** (`wg review consume`, MUST-2) re-hashes the presented bytes
  and **permits the exact reviewed bytes but refuses a post-review mutated byte** (the RA-8
  TOCTOU close). **Taint-inference fail-closed routing** (RA-9) overrides a self-asserted
  `low` secret-touching task **upward to high**. The **loud revoke leg** (`wg review revoke`,
  ADR-CS3 D4) traces a later-discovered poison by its content digest, **lowers the author's
  trust** (so its next item takes the deep path), and **names the downstream consumers to
  re-run** — the safety guarantee is the containment + audit + revoke leg, not detection.

CLI: `wg review check` (screen one item through Pass 0→2 and record a verdict) / `depth`
(show the applied trust×sensitivity depth) / `reviewer-scope` / `log` / `consume` / `revoke`.
**Spark boundary** (`docs/content-safety-study/04` §4.3) — Pass 2 here is a **deterministic**
semantic classifier, *not* a live weak-tier LLM call, so the smoke gate runs
**credential-free**; the spark proves the **slot and the structural bounds** (no-scope,
spotlight, quorum, structured verdict), not the silicon. The production weak-tier `.review-*`
one-shot (`resolve_agency_dispatch`), the N-reviewer + model-strength-by-depth ladder, the
real Pass-3 sandbox, the full cross-plane D-iii TC8 defense, and the human at Pass 4 are
**Review-Wave C/D**. The pipeline is reachable today through the `wg review` CLI;
**auto-wiring the four ingest seams** into the live import / accept / msg / state-load paths
is the production build, not the spark. **The IC4 (message) ingest seam is now wired**:
`wg msg poll --as <id> --review` (and `wg identity poll --review`) auto-screen each
*authenticated* inbound through the review pipeline at the consumption edge (received ≠
consumed) and **refuse consumption of a non-accept verdict**, with author-trust **derived**
(no `--trust` flag) from the canonical `worksgood::trust::resolve_author_trust` dial —
which unifies the federation **peer registry** (`wg peer add --trust`, a new `PeerConfig.trust`
field) with the **WG-Exec provider pool** (`exec/registry.json`, via the one
`ProviderRegistry::load`), fail-closed to `Unknown`. So review *depth* and the exec *leash*
read **one** trust dial. The remaining import / accept / state-load seams stay Review-Wave C/D.

The end-to-end proof is pinned by `tests/smoke/scenarios/content_safety_spark.sh`
(`owners = [cs-spark]`): a legit `Verified` low-sensitivity IC1 takes the light path and is
**accepted** (the must-not-over-block bound) → a hostile IC1 prompt-injection from an
`Unknown` author is **rejected before any agent consumes it** → a poisoned IC2 diff that
passes its own tests but plants a backdoor is **rejected at the accept seam** → the
inject-the-reviewer attempt is **contained** (no verdict flip, no action) → depth is
trust-proportional (Verified+low light, Unknown deep/quarantine) → fail-closed taint +
digest-pin → detect-contain-revoke traces the author from the verdict sigchain, lowers trust,
and names the consumer to re-run.

## WG-Exec — the execution-federation plane (`wg provider`, Exec Spark)

WG-Exec is the **execution-federation** layer decided in
`docs/execution-federation-study/06` (§4 the spark, §5 Exec-Wave B) and ratified in
`docs/ADR-exec-e1..e4-*.md` (indexed by `docs/ADR-exec-000-acceptance-brief.md`). The
**Exec-Wave B spark** (`src/providers/` + `wg provider` in `src/commands/exec_fed_cmd.rs`)
is the first execution-plane code: the thinnest end-to-end slice that proves *"one task,
a borrowed box, a scoped leash"* — a task placed on a **separately-owned remote provider**
runs under **two scoped UCANs (never the agent's root key)**, reads only its sealed slice,
and signs a result back, with the provider demonstrably **unable to exceed its lease** and
a hostile provider's corrupted result **caught by a disjoint re-run vs a pinned spec**.

It **composes with WG-Fed and invents no second trust system** (NFR-4): identity (`wgid:`),
the sigchain, the custodian-held-root signing boundary, the **attenuating UCAN**
(`identity::custody`), and the **per-recipient sealed envelope** are all reused verbatim.
WG-Exec owns only the **execution wire** (the five envelopes), the **lease-epoch fence**,
and **how the leash dial wires UCAN scope/TTL + lease term/cadence together per task**. Its
prerequisite is the WG-Fed spark (`federation_spark_two_graphs`) passing first.

- **`src/providers/`** — `mod.rs` (`WG_EXEC_COMPAT_VERSION`, an **authenticated loud-fail**
  handshake mirroring `WG_FED_COMPAT_VERSION`; the **five wire envelopes**
  `PlacementOffer` / `Claim` / `RunGrant` / `LeaseRenewal` / `ResultEnvelope`; the
  `ProviderRegistry` whose `ProviderEntry::is_live` is **signed-renewal** liveness, never a
  local PID; `TrustLevel` is `graph::TrustLevel` — the one trust dial). `placement.rs` — the
  matcher (**hard filter** capability+trust-floor → **advisory deterministic rank**) and the
  **fail-closed `leash()` engine** (confidential ⇒ attested-C-or-refuse, **unlabeled ⇒
  refuse, never A**). `bundle.rs` — build / seal / verify the minimal `ContextScope` slice
  over WG-Fed crypto (encryption=ACL). `lease.rs` — the signed `Lease` + the **monotonic
  epoch atomic-CAS fence** at the single canonical-write boundary (closes the double-commit
  / replay). `verify.rs` — attribution (mandatory but **not** integrity) + a **single
  disjoint re-run in a trusted domain (never the producer — X-5) vs a *pinned* spec (not the
  provider's shipped tests — X-6)**.
- **Dispatch seams** — `handler_for_model.rs`'s `ExecutorKind` gains a **`RemoteRunner`** arm
  (driven by the providers plane, not the local spawn-task handler) and `plan_spawn`'s
  `SpawnPlan` gains a **`placement` field** (`Placement::{Local, Provider(wgid:)}`; `Local`
  reproduces today's spawn byte-for-byte). The capability-gated claim is `wg provider claim`
  (the four-part eligibility proof); the local `wg claim` is unchanged (a local worker
  trivially clears the trusted-pool filter).
- **The two scoped UCANs reuse WG-Fed's UCAN** verbatim: an *act-as-agent* UCAN
  (`act-as-agent` on `agent://<G>/task/<T>`) and a *graph-write* UCAN (`graph/write` on
  `graph://task/<T>` only — **never** `graph://*`), both issued via `custody::issue_root`
  with the leash-decided scope/TTL. A `RunGrant::field_scan` asserts **no root key, no
  blanket graph-write** in the delivered bytes.

```
wg provider enroll <wgid> --trust verified --model claude:opus --isolation container
wg provider offer  --as-name <agentG> --task T --model … --isolation … [--sensitivity normal|high|confidential] --provider <wgid> --out offer.json
wg provider claim  --as-name <providerP> --offer offer.json --store <L> --out claim.json
wg provider grant  --as-name <agentG> --claim claim.json --task-input <file> [--after dep=path] [--ucan-ttl-secs N] --store <L> --out grant.json
wg provider run    --as-name <providerP> --grant grant.json --store <L> --out result.json [--target-task U] [--corrupt] [--scope-probe <secret>]
wg provider accept --result result.json --store <L> [--now <rfc3339>]
wg provider reclaim --task T [--new-provider <wgid>]      # bump the lease epoch
wg provider verify --result result.json --verifier <Q> --pinned-spec spec.json --store <L>
wg provider show <…> | wg provider providers              # surface the applied leash + liveness
```

Compatibility is gated by `WG_EXEC_COMPAT_VERSION` in `src/providers/mod.rs` (loud-fail on
incompatible mismatch). The six-step end-to-end proof is pinned by
`tests/smoke/scenarios/exec_spark_borrowed_box.sh` (`owners = [exec-spark]`): place on a
separately-owned provider (no root / no blanket write in the grant) → run under the scoped
UCAN reading only its slice → sign a result back (wrong-signed rejected) → the provider
cannot exceed its lease (wrong-task write / post-expiry sign / replay + stale-after-reclaim
all rejected) → a hostile provider's corrupted + test-poisoned result is caught by a disjoint
re-run vs the pinned spec → a confidential task to a non-attested provider is refused, never
shipped in plaintext (fail-closed). **Spark boundary** (memo §4.3): the attestation slot is
exercised by the **fail-closed refuse**, not a real enclave; the integrity lever is a
**single disjoint re-run**, not quorum; the pinned-spec re-run + `auto_evaluate` eval-gate are
deterministic stubs — the real enclave (Exec-Wave D), the verified-overflow B tier, and the
production weak-tier re-run are later waves.

## WG end-to-end — the family-team integration (`tests/smoke/scenarios/e2e_family_team.sh`)

The three sparks (WG-Fed, WG-Review, WG-Exec) each pass in **isolation**. The **family-team
e2e** (`e2e_family_team.sh`, `owners = [e2e-family-team]`) is the milestone that proves they
**COMPOSE** into one continuous flow across **two FS-independent instances** — distinct `$HOME`
keystore + distinct `--dir` graph, **no shared filesystem**, whose ONLY channel is a dumb,
untrusted HTTP relay node (`wg fed-node serve`; every byte self-verifying). It reuses the
existing identity/UCAN/seal substrate with **no second trust system** and adds no new compat
const. Cast: instance A (the family home) = **Sara** (human requester) + **Luca** (operates the
borrowed compute box, the WG-Exec Provider P); instance B (the chef host) = **Bruno** (chef
agent, principal/authorizer — root custodied on B, never leaves it; accepts + verifies) +
**Nora** (dietitian agent — the disjoint integrity verifier Q ≠ producer); plus **Mallory** (a
stranger adversary).

The continuous chain (each link a falsifiable assertion): **(1) identity** — mint the four
family `wgid:`s, cross-publish + cross-fetch + OFFLINE-verify across the wall, no private key in
any published byte; **(2) cross-graph task** — Sara sends "plan Wednesday dinner" sealed to Bruno
via `wg msg --to wgid:`, Mallory plants a hostile variant, Bruno polls his node inbox, both
authenticate by key, a forged "from Sara" is rejected; **(3) review gate on the way IN**
(received ≠ consumed) — Bruno screens each inbound BEFORE consuming: Sara's legit task is accepted
on the light path and becomes the exec input while Mallory's planted hostile variant is
quarantined/rejected and **never consumed** (no exec offer is made for it); **(4) remote exec** —
Bruno places the reviewed task on the OTHER instance's compute (Luca's borrowed box on A) under
**two scoped attenuating UCANs** (act-as-agent + graph-write scoped to `graph://task/wed-dinner`)
— never his root, never a blanket write — and the box opens ONLY its `task` slice; **(5) signed
result back** — Bruno (authorizer) accepts + verifies the signed result against HIS sigchain
(attributed to Bruno; wrong-signed rejected), the borrowed box cannot exceed its lease
(wrong-task / post-expiry / replay / stale-after-reclaim all fenced), a corrupted plan is caught
by Nora's disjoint re-run vs the pinned spec, a confidential task to the non-attested box is
refused fail-closed, and the signed completion crosses the wall **back** to Sara on instance A,
authenticated as Bruno.

**Seam finding.** The three modules compose cleanly because they share one substrate (the e2e
required **no production-code change**). The one real seam the isolated sparks could not surface
is **auto-wiring**: the review gate's author-trust is hand-passed (`--trust`) and the gate is
invoked manually between `wg msg poll` and the exec offer. Production must (a) derive author-trust
canonically from the federation peer/sigchain `graph::TrustLevel` (the same dial the exec pool
reads) and (b) auto-run the review pipeline at the live ingest edge so "received ≠ consumed" holds
with no manual step. **This is now SHIPPED for the IC4 (message) ingest path** (the `auto-wire-the`
build): `src/trust.rs::resolve_author_trust` is the canonical trust resolver unifying the federation
peer registry (`wg peer add --trust`) with the WG-Exec provider pool, and `wg msg poll --review`
auto-screens each authenticated inbound through the review pipeline with that *derived* trust,
refusing consumption of a non-accept verdict (no `--trust` flag, no separate `wg review check`).
It is pinned by `tests/smoke/scenarios/e2e_autowire_ingest_gate.sh` (`owners = [auto-wire-the]`).
The remaining import / accept / state-load ingest seams stay Review-Wave C/D. **Spark boundary**:
the exec result work-product is the exec spark's deterministic stub (the
real weak-tier LLM is a later wave); the e2e proves the **composition + the security bounds at
every seam**, not the silicon.

## WG-Pilot — turnkey family-team deploy (`wg pilot`, the deploy/UX wrapper)

WG-Pilot is the **one-command stand-up** of the real family-team federation on real
machines — the deploy/UX wrapper over the verified WG-Fed + WG-Review + WG-Exec substrate
(`docs/prod-audit/01`). It **ships NO new substrate and no new compat const**: it sequences
the existing `wg identity` / `wg fed-node` / `wg peer` / `wg msg` / `wg review` /
`wg provider` verbs and applies the SAFE defaults. It targets the **verified v1 profile** —
configured-peer, non-confidential-remote, block-don't-triage (no DHT, no TEE, no
human-in-loop). Cast: humans **Sara** + **Luca** (home host), agents **Bruno** (chef,
authorizer) + **Nora** (dietitian, disjoint verifier) (chef host), each a `wgid:` identity.

- **`src/commands/pilot_cmd.rs` + `wg pilot` (`Commands::Pilot`)** — `up` / `status` / `down`.
  From a filled config it mints the 4 identities into `wg secret` custody, starts the
  `wg fed-node` inbox, wires the configured cross-host peers (`wg peer add --wgid --endpoint`)
  with **split trust** (family Verified, everyone else Unknown), applies the **fail-closed /
  slack-bounded-leash / confidential-refuse / configured-peer** defaults (an explicitly-unsafe
  knob is **refused loudly before anything is stood up** — `resolve_safe_defaults`), optionally
  wires each agent's Telegram bot (`[telegram.bots.<name>]`, the multi-bot feature), and runs a
  **live end-to-end check** (task crosses the wall → content-review gate blocks an Unknown
  injection while the Verified task is consumable → borrowed-box exec under two scoped UCANs,
  no root/no blanket write → signed result accepted + attributed, wrong-signed rejected →
  confidential-to-non-attested REFUSED). Orchestration is by spawning `wg` itself
  (`current_exe`) per role with an isolated `$HOME` keystore + `--dir` graph (the smoke
  `wgrun` pattern). `down` SIGTERMs the node + clears state and is **idempotent**;
  `--wipe-identities` also wipes the rehearsal keystore (real deploys keep custodied roots).
- **`pilot.example.toml`** — the operator-supplied bits ONLY (host bind/endpoint, OpenRouter
  key path, optional Telegram tokens, trust defaults); everything else defaults SAFE. Kept in
  lock-step with the parser by a unit test (`shipped_example_template_parses_and_is_safe`).
- **`wg pilot up --dry-run`** — the smoke-tested rehearsal: models BOTH hosts locally as two
  FS-isolated dirs sharing one relay node and runs the whole family-team live check
  credential-free. Real multi-host `wg pilot up --config pilot.toml` runs once per host
  (`[pilot].role = home|chef`), wiring pre-exchanged `[[peers]]` wgids + probing peer health.
- **Operator README** — `docs/ops/runbook.md` §6 (what you provide vs automated, the one
  command, verify, teardown).

The end-to-end proof is pinned by `tests/smoke/scenarios/pilot_dry_run.sh`
(`owners = [pilot-deploy]`): one command stands up the pilot and the live family-team check
passes (`check_passed=true`, 4 identities minted, relay `/health` reachable) → the SAFE
defaults are applied (no unsafe knob on by default) → teardown stops the node (recorded pid
gone) and is idempotent → an explicitly-unsafe config is refused before stand-up. **Spark
boundary**: the dry-run worker + Pass-2 reviewer are deterministic/credential-free; the
live-model tier + full cross-host silicon ride the same wiring in a real deploy.
