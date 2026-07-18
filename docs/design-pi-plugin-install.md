# Design: WorksGood Pi install (`@worksgood/pi`, display `pi-worksgood`)

**Status:** implemented. The operational compatibility command remains
`wg pi-plugin`; it is not the package or display identity.

**Canonical identity:** npm `@worksgood/pi`; Pi startup label `pi-worksgood`;
repository component `worksgood-pi/`; cache component `worksgood-pi`; executable
`wg`. The approved description is “Connect Pi agents to WorksGood graphs,
tools, and context.”

**Scope:** make *"pi is correctly wired to wg"* a declarative, idempotent
consequence of choosing pi — never a manual step that drifts. This document
resolves every open decision and hands the impl task a turnkey plan.

---

## 0. The problem this kills

Every manual / copy-based mechanism in this repo drifts:

- stale `wg` binary copies on `PATH`,
- the stray `WG_EXECUTOR_MODEL_MISMATCH.md`,
- a glm-5.2 401 where the handler pointed at a route with no credential and the
  failure was invisible until an agent died at ~1s.

At the time this design was adopted, the WorksGood Pi integration made this
worse by construction:
`worksgood-pi/src/wg-backend.ts`
states *"Today every call shells out to the `wg` binary via `pi.exec('wg', …)`"*,
so a **version-skewed plugin silently sends the wrong flags** to whatever `wg` is
on `PATH`. And the install was *absent*:

- `~/.pi/agent/extensions/` is empty (no global plugin),
- there was **no `wg pi-plugin install` command** (`wg skill install` supplied
  the precedent — `src/commands/skills.rs::run_install`),
- `pi-handler`'s Topology-A spawn (`src/commands/pi_handler.rs::rpc_spawn_args`)
  passed **no** `-e`/`-ne` flags, so it relied on an ambient global extension that
  did not exist. The handler comment claimed the extension was *"installed in
  `~/.pi/agent/extensions/`, via `pi -e <plugin>`, or settings `packages`"* — none
  of which is actually arranged. That gap is the drift.

## 1. Core principle — ONE idempotent primitive

There is exactly **one** primitive, `ensure-pi-plugin`, invoked declaratively at
every lifecycle point where pi becomes relevant. Not three install paths that
drift.

`ensure-pi-plugin` is a pure function of *(the running `wg` binary, the
filesystem)*:

> resolve the canonical plugin build → place it → write the settings / extension
> entry (console direction only) → verify it loads **and** its compat version
> matches the `wg` binary → **no-op if already correct, repair if drifted**.

It must be fast (a few `stat`s on the happy path), headless-safe (never blocks on
a prompt), offline-capable, and self-healing.

The linchpin that makes "version match" true *by construction* rather than by
hope: **the `wg` binary carries the exact plugin build it is compatible with**,
vendored and embedded at build time. There is no PATH/npm skew to reason about,
because the bytes the plugin runs from came out of the same binary that will
shell back to `wg`.

## 2. Two directions need different things

The extension is loaded in two fundamentally different situations. Conflating them
is how the current design drifts.

### 2.1 `wg → pi` (handler/worker — the OSS-execution path): **HERMETIC**

WG spawns pi. `wg pi-handler` launches pi pointed at the **embedded build** (a
versioned cache path), telling pi exactly where its own plugin lives. **No global
`~/.pi` install is needed or touched.** This direction must become *impossible to
get wrong*: the binary that spawns pi is the same binary whose embedded bytes the
plugin runs.

### 2.2 `pi → wg` (human runs a pi console): **GLOBAL**

A human launches `pi` themselves (Topology C, the attended front-end). pi must
auto-discover the plugin, so this direction *does* need the global
`~/.pi/agent/settings.json` entry (and/or the global extensions dir). **Only this
direction touches `~/.pi`.**

`ensure-pi-plugin` therefore takes a **mode**:

| Mode | Caller | Touches `~/.pi`? | Output |
|---|---|---|---|
| `Hermetic` | `wg pi-handler` (JIT, pre-spawn) | **no** | a `ResolvedPlugin` the handler passes to pi via `-e` (Topology A) or the Node host (Topology B, dev) |
| `Console` | `wg setup`, `wg profile use pi`, `wg pi-plugin install` | **yes** | the cache bundle + a global pi settings `extensions` entry pointing at it |

Both modes share the same "make the versioned cache correct" core; only `Console`
additionally writes the pi-side settings entry.

---

## 3. Open decisions — RESOLVED

### Decision 1 — Build / vendor mechanism → **(b) commit a prebuilt, curated embed; CI guards staleness**

**Options considered:**

- **(a) `build.rs` runs `npm run build` then `include_bytes!`s `pi-worksgood/`.**
  *Rejected.* This forces a working `node` + `npm` toolchain at **`cargo install`
  time**, which breaks node-less installs of `wg`. The repo's whole point is that
  `cargo install --path .` is node-free (CLAUDE.md "Development"). It also makes
  build reproducibility depend on the installer's node version.

- **(b) commit a prebuilt, curated plugin bundle into the repo, regenerated by a
  `make`/`xtask` step, and `include_*!` the committed artifact.** **RECOMMENDED.**
  Keeps `cargo install --path .` node-free (the bytes are already in the tree),
  works offline, and makes the embedded build a reviewable artifact. This is
  exactly the shape of the existing `wg skill install` precedent, which
  `include_str!`s `.claude/skills/wg/SKILL.md` (`src/commands/skills.rs:7`).

**How (b) is wired (and how staleness is *prevented*):**

1. **Committed embed dir** — `worksgood-pi/embedded/` (tracked; distinct from the
   gitignored dev `worksgood-pi/pi-worksgood/` in `worksgood-pi/.gitignore`). It contains only
   the **runtime** files, so diffs stay small and the binary stays lean:

   ```
   worksgood-pi/embedded/
     pi-worksgood/index.js    # + sibling modules imported by index.js:
     pi-worksgood/wg-backend.js
     pi-worksgood/tools.js
     pi-worksgood/commands.js
     pi-worksgood/model-bridge.js
     pi-worksgood/graph-widget.js
     host/wg-pi-host.mjs      # Topology-B host (verbatim copy)
     package.json             # curated: name, version, type:module, pi.extensions, peerDeps
     version.json             # { "compat": "<WG_PI_PLUGIN_COMPAT_VERSION>" } — the wire-compat stamp
   ```

   `.d.ts`, `*.map`, `*.tsbuildinfo` are **excluded** (not needed at runtime;
   they make diffs noisy and bloat the binary).

2. **Regeneration target** — `make embed-worksgood-pi` (thin wrapper over a
   `cargo xtask` or a shell script) does:
   `npm --prefix worksgood-pi ci && npm --prefix worksgood-pi run build`, then copies
   the curated subset into `worksgood-pi/embedded/` and stamps `version.json` from
   the Rust `WG_PI_PLUGIN_COMPAT_VERSION` const (§Decision 1 single-source rule
   below).

3. **Embed into the binary** — use the `include_dir` crate (small, widely used)
   to embed `worksgood-pi/embedded/` as a compile-time directory:
   `static EMBEDDED_PI_PLUGIN: include_dir::Dir = include_dir!("$CARGO_MANIFEST_DIR/worksgood-pi/embedded");`.
   *No-new-dependency fallback:* a generated `embedded_pi_plugin.rs` holding a
   `&[(&str, &[u8])]` slice produced by the `make` step (mirrors how `skills.rs`
   uses `include_str!`, just for many files). Recommend `include_dir` for
   ergonomics; the slice is acceptable if adding the dep is undesirable.

4. **CI staleness gate** (the anti-drift guarantee) — a CI job rebuilds the
   plugin into a temp dir with the *same* curated copy logic and asserts it is
   byte-identical to the committed `worksgood-pi/embedded/` (effectively
   `make embed-worksgood-pi && git diff --exit-code worksgood-pi/embedded`). If a dev
   edits `worksgood-pi/src/**` without re-running the embed step, CI fails. This is
   what makes "committed runtime == fresh build" an invariant, not a hope.

   *Determinism note:* `tsc` output is deterministic given the pinned
   `package-lock.json` + the `rust-toolchain`-style pin already in the repo;
   excluding source maps removes the only path-dependent output. If a future tsc
   bump perturbs whitespace, the CI diff catches it and the fix is a re-embed
   commit.

### Decision 2 — How pi loads a path-based extension → **VERIFIED against pi 0.79.10 CLI + bundled SDK docs**

This was verified empirically, **not assumed**, against the `pi` binary installed
on this machine (`pi --version` → `0.79.10`) and pi's own bundled docs at
`…/@earendil-works/pi-coding-agent/docs/extensions.md`.

**What `pi` supports (from `pi --help`, pi 0.79.10):**

```
--extension, -e <path>      Load an extension file (can be used multiple times)
--no-extensions, -ne        Disable extension discovery (explicit -e paths still work)
pi install <source> [-l]    Install extension source and add to settings
pi remove / uninstall <source> [-l]
pi list                     List installed extensions from settings
```

- `pi install ./local/path` accepts a **local path**; `-l`/`--local` writes the
  **project** `.pi/settings.json`, default writes the **global**
  `~/.pi/agent/settings.json` (verified via `pi install --help`).

**Auto-discovery + settings schema (from `docs/extensions.md`):**

| Location | Scope | Trust gate |
|---|---|---|
| `~/.pi/agent/extensions/*.ts`, `~/.pi/agent/extensions/*/index.ts` | global | **none** |
| `.pi/extensions/*.ts`, `.pi/extensions/*/index.ts` | project-local | **trusted-project only** |

```jsonc
// ~/.pi/agent/settings.json  (the real global settings file on this box)
{
  "packages":   ["npm:@foo/bar@1.0.0", "git:github.com/user/repo@v1"],
  "extensions": ["/abs/path/to/extension.ts", "/abs/path/to/extension/dir"]
}
```

Extensions are loaded via **jiti** (TypeScript loads without compilation; plain
`.js` loads too — our built artifact is `pi-worksgood/index.js`). pi-core packages stay
in `peerDependencies: "*"` and are **provided by pi at load**, so the embedded
bundle carries *no* `node_modules`.

**Exact mechanism each direction uses:**

- **`wg → pi` hermetic, Topology A (primary):**
  `wg pi-handler` spawns
  `pi --mode rpc -e <cache>/<compat>/pi-worksgood/index.js -ne …`.
  `-e` loads *exactly* the embedded build by absolute path; `-ne` disables **all**
  discovery (no `~/.pi` global, no project `.pi`, and — deliberately — none of the
  user's other global `packages` like `pi-web-access`). Per the help text, "*explicit
  -e paths still work*" under `-ne`. Result: **fully hermetic, offline, node-free,
  version-matched by construction**, needing only the `pi` binary (which bundles
  the pi SDK that satisfies the plugin's peer imports). This is the single line
  the impl task adds to `rpc_spawn_args`, and it closes the drift.

- **`pi → wg` global console:**
  `ensure-pi-plugin` (Console mode) writes an idempotent **`settings.json`
  `extensions` entry** — an absolute path into the versioned cache:
  `"extensions": ["<cache>/<compat>/pi-worksgood/index.js"]` in
  `~/.pi/agent/settings.json`. Chosen over the alternatives because it is
  offline, needs no `npm`, sidesteps the project trust gate (it is the *global*
  settings file), and is trivially idempotent. Documented alternatives:
  `pi install <cache>/<compat>` (blessed, but runs a production `npm install
  --omit=dev` and may hit the network) and a symlink at
  `~/.pi/agent/extensions/pi-worksgood` (the `*/index.ts` discovery form — note
  the table globs `.ts`, so prefer the settings entry for our `.js` artifact).

- **Topology B (`node wg-pi-host.mjs`) loading** is unchanged in mechanism: the
  host loads the bundle **in-process** via
  `DefaultResourceLoader({ extensionFactories: [wgPlugin], eventBus })`
  (`worksgood-pi/host/wg-pi-host.mjs`). This is *not* a path/CLI load and is not the
  hermetic primary — see the §4.2 caveat (it needs the pi SDK resolvable via
  `node_modules`, which only the in-repo dev tree has).

**Fallback if a future pi dropped launch-time path load:** there is none needed —
`-e`/`-ne` are present and stable in 0.79.x. If a hypothetical future pi removed
`-e`, the fallback is to pre-write the global `settings.json` `extensions` entry
(Console mechanism) **before** the Topology-A spawn and rely on auto-discovery —
losing hermeticity but preserving correctness. The impl task should pin the
verified flags and add a one-line capability probe (`pi --help | grep -- -e`) so
a future regression fails loudly rather than silently.

### Decision 3 — Cache location + invalidation → **`${XDG_CACHE_HOME:-~/.cache}/wg/worksgood-pi/<compat-version>/`**

- **Path:** `${XDG_CACHE_HOME:-$HOME/.cache}/wg/worksgood-pi/<compat-version>/`,
  honoring `XDG_CACHE_HOME` (the repo has no existing cache helper — grep finds
  none — so introduce a tiny `wg_cache_dir()` that resolves
  `XDG_CACHE_HOME` then `$HOME/.cache`, falling back to a temp dir if neither is
  writable, for headless/CI safety). The `<compat-version>` segment is the
  invalidation key: a `wg` binary with a new compat version writes a **new**
  directory and never collides with an old one.
- **Layout under the version dir:** mirrors the embed — `pi-worksgood/`, `host/`,
  `package.json`, `version.json`, plus a `.wg-ok` integrity stamp written **last**.
- **Atomic extraction:** extract into a sibling temp dir
  (`…/worksgood-pi/.<compat>.tmp-<pid>`), `fsync`, then `rename` into place. A crash
  mid-extract leaves a `.tmp-*` dir (GC'd) and **no** `.wg-ok` in the live dir, so
  the next `ensure` re-extracts. This prevents a half-written cache from being
  trusted.
- **GC of stale versions:** on every `ensure`, best-effort remove sibling
  `…/worksgood-pi/<other-version>/` directories whose name ≠ the current compat
  version, and any leftover `.tmp-*`. Skip removal on any error (never fatal). The
  cache is disposable: deleting it entirely just forces a re-extract on next use.

### Decision 4 — Dev vs user source of truth → **dev tracks live `worksgood-pi/pi-worksgood`; user uses embedded→cache**

`ensure-pi-plugin` selects the source in this precedence:

1. **Explicit override:** `WG_PI_PLUGIN_DIR` set → use it verbatim (already an
   honored env in `executor_discovery::pi_plugin_candidate_dirs`). Escape hatch
   for testing a hand-built bundle.
2. **Dev (repo present):** if the compile-time `worksgood-pi/` tree exists *and*
   looks like the wg repo (guard: `worksgood-pi/package.json` parses with
   `"name": "@worksgood/pi"`) *and* `worksgood-pi/pi-worksgood/index.js` exists →
   **point at the live `worksgood-pi/pi-worksgood`** so it tracks the working build (a
   `npm run build` is reflected immediately, no re-embed). This matches
   `pi_plugin_candidate_dirs`' "in-repo first" precedence and keeps the dev
   inner-loop tight. *Caveat documented:* the in-repo `pi-worksgood` build can be stale if the
   dev forgot to rebuild; the compat tripwire (§Decision below / §5) catches a
   *version* mismatch but not a same-version stale build — devs must
   `npm run build`. (`make embed-worksgood-pi` and a `/reload` in attended pi are the
   two ways to refresh.)
3. **User (cargo-installed, no repo):** the compile-time `worksgood-pi/` path is
   absent (or fails the repo guard) → **extract the embedded bundle into the
   versioned cache** and use that. Node-free, offline, version-locked.

The dev branch only ever wins for an *actual wg checkout*; the repo-name guard
prevents a `cargo install`ed binary whose baked `CARGO_MANIFEST_DIR` happens to
still exist from accidentally loading an unrelated tree.

### Decision 5 — Idempotency + self-heal contract → **"already correct" is a 3-(or-4-)part predicate; repair = re-materialize the embedded truth**

For a target location `T` (the chosen cache version dir, or the dev build),
`ensure-pi-plugin` treats `T` as **already correct** iff **all** hold:

1. **Present:** `T/pi-worksgood/index.js` exists (and, when Topology B is in play,
   `T/host/wg-pi-host.mjs`).
2. **Version-matched:** `T/version.json`'s `compat` equals the binary's
   `WG_PI_PLUGIN_COMPAT_VERSION`. (For the dev source, the version comes
   from the built bundle's stamp; a mismatch means "rebuild/re-embed".)
3. **Intact:** the `.wg-ok` integrity stamp is present (proves a complete atomic
   extraction). *(The dev build is exempt from the stamp — it is managed by `tsc`,
   not by extraction; presence + version are sufficient there.)*
4. **(Console mode only) Wired:** `~/.pi/agent/settings.json` contains an
   `extensions` entry whose absolute path resolves to `T/pi-worksgood/index.js` (or the
   current cache `pi-worksgood/index.js`).

If all hold → **no-op** (cost: a handful of `stat`s + one small JSON read).
Otherwise **repair**, always by *re-materializing the embedded truth* (never
partial patching of suspect bytes):

- Present/intact/version fails (cache) → atomic re-extract of the embedded bundle
  (§Decision 3), rewrite `version.json` + `.wg-ok`.
- Console wiring fails → rewrite the single `extensions` entry idempotently
  (replace any existing wg-managed entry; leave the user's other `extensions`/
  `packages` untouched).
- Always GC stale sibling versions.

Repair is convergent: running `ensure-pi-plugin` twice in a row leaves the second
run a pure no-op.

### Legacy settings migration (rename-pi-integration)

Console ensure recognizes both prior installation forms:

- npm `@worksgood/wg-pi-plugin`, including version-pinned strings and object
  entries with Pi package filters;
- managed paths under `…/wg/pi-plugin/<compat>/dist/index.js` or an in-repo
  `pi-plugin/dist/index.js`.

Pi treats npm packages and direct extension paths as distinct identities, and a
legacy package cannot be safely rewritten to `@worksgood/pi` while offline: the
new package may not be installed yet, which would leave the console with no
WorksGood tools. Console ensure therefore retains each legacy/canonical package
record and version pin, converts it to Pi's object form when necessary, and sets
its `extensions` filter to `[]`. It then removes stale managed paths and loads
exactly one compat-locked
`…/worksgood-pi/<compat>/pi-worksgood/index.js`. The old package remains inert
until the user removes it with `pi remove npm:@worksgood/wg-pi-plugin`.

The first legacy-package acceptance prints that actionable compatibility notice;
subsequent idempotent ensures are silent. Unrelated Pi packages, extension paths,
ordering, and package configuration remain intact. This simultaneously preserves
offline operation, prevents duplicate tool/command registration, and guarantees
that Pi derives the public `pi-worksgood` label from the managed entry's parent.

### Compat handshake — `WG_PI_PLUGIN_COMPAT_VERSION` (mirrors `WG_AGENCY_COMPAT_VERSION`)

Reuse the established compat-version precedent:
`src/agency/mod.rs` defines `pub const WG_AGENCY_COMPAT_VERSION: &str = "1.2.4";`
(threaded through `agency_bridge` / `agency_import` / `agency_stats`). Introduce a
sibling:

```rust
// e.g. src/pi_plugin/mod.rs (new module) — single source of truth
pub const WG_PI_PLUGIN_COMPAT_VERSION: &str = "0.2.0";
```

**Single-source rule:** this Rust const is authoritative. `make embed-worksgood-pi`
stamps it into `worksgood-pi/embedded/version.json`, and the plugin build stamps the
**same** value into the bundle (a generated `src/version.ts` written from the
const at embed time, or read from `package.json` with a unit test asserting
equality). A Rust unit test asserts
`WG_PI_PLUGIN_COMPAT_VERSION == embedded/version.json.compat` so they can never
silently diverge. (This is a *wire-compat* number, deliberately decoupled from the
npm `package.json` `version` of `@worksgood/pi` — exactly as agency's
`1.2.4` is decoupled from any package version. Bump it whenever the wg↔plugin
flag/contract surface changes.)

**Runtime assertion (the loud-fail tripwire), in the plugin factory
(`worksgood-pi/src/index.ts`):**

```
EXPECTED = process.env.WG_PI_PLUGIN_COMPAT_VERSION         // wg→pi: injected at spawn
        ?? (await pi.exec("wg", ["pi-plugin", "compat-version"])).stdout.trim()   // pi→wg: ask the wg on PATH
if (EXPECTED && EXPECTED !== EMBEDDED_COMPAT)
    throw new Error(`WorksGood Pi integration compat mismatch: extension=${EMBEDDED_COMPAT} wg=${EXPECTED}; run \`wg pi-plugin install\``)
```

- **`wg → pi`:** `wg pi-handler` injects `WG_PI_PLUGIN_COMPAT_VERSION` into the
  child env at spawn (cheap, no extra `exec`). Because the binary writes its own
  embedded bundle to the cache and points pi at it, this is a *tripwire*, not the
  guarantee — the guarantee is the embed itself.
- **`pi → wg` (human console):** the env is absent, so the plugin shells
  `wg pi-plugin compat-version` against the wg actually on `PATH` and compares.
  This is the real drift catcher for the console direction (newer wg / older
  global plugin, or vice versa).
- **Failure surfacing:** a thrown factory error is collected by the SDK as an
  extension load error — the Node host already reads `extensionsResult.errors`
  (`worksgood-pi/host/wg-pi-host.mjs` `collectRegistrations` / `runSelftest`), and
  attended pi shows the load error in its UI. The `wg pi-plugin compat-version`
  verb is added alongside `wg pi-plugin install` (§4.3).

`WG_DAEMON_SOCKET` stays **dormant**: `wg-backend.ts` already stubs the
future daemon-IPC seam; this design is about *install*, not transport, and does
not activate it.

---

## 4. Architecture

### 4.1 `ensure-pi-plugin` contract (the one primitive)

```
fn ensure_pi_plugin(mode: EnsureMode) -> Result<ResolvedPlugin>

enum EnsureMode { Hermetic, Console }

struct ResolvedPlugin {
    root: PathBuf,            // the version dir (cache) or the dev worksgood-pi/ root
    dist_entry: PathBuf,      // <root>/pi-worksgood/index.js  — for `pi -e`
    host_script: PathBuf,     // <root>/host/wg-pi-host.mjs — for Topology B
    compat: String,           // WG_PI_PLUGIN_COMPAT_VERSION it materialized
    source: Source,           // Dev | Cache | EnvOverride
    has_node_modules: bool,   // true only for the in-repo dev tree (see §4.2)
}
```

Steps (both modes): pick source (§Decision 4) → if Cache source, make the
versioned cache correct via the §5 predicate + atomic repair → (Console only)
write/refresh the global settings `extensions` entry (§Decision 2) → return
`ResolvedPlugin`. Always headless-safe and non-interactive; never prompts.

### 4.2 Hermetic spawn vs global console — the split, concretely

**`wg → pi` (Hermetic), in `pi_handler::run` before `select_topology`:**

```
let plugin = ensure_pi_plugin(Hermetic)?;     // JIT safety net (wiring point #3)
// inject the tripwire env for the child:
//   WG_PI_PLUGIN_COMPAT_VERSION = plugin.compat
//   WG_PI_PLUGIN_DIR            = plugin.root   (so executor_discovery + host agree)
```

Topology selection becomes plugin-aware:

- **Topology A (primary, hermetic):** `pi` binary present →
  `pi --mode rpc -e <plugin.dist_entry> -ne …`. The impl task adds `-e
  <dist_entry>` + `-ne` to `rpc_spawn_args` (which today passes neither). Needs
  only `pi`; the pi binary supplies the plugin's peer deps. Offline, node-free,
  version-locked.
- **Topology B (dev convenience only):** chosen when there is **no** `pi` binary
  (or `WG_PI_TOPOLOGY=node`) **and** `plugin.has_node_modules` (i.e. the in-repo
  dev tree, which has `worksgood-pi/node_modules` from `npm install`). A *cache-only*
  Topology B is **out of scope**: a bare `node` cannot resolve the pi SDK
  (`@earendil-works/pi-*`) without `node_modules`, and we deliberately do not
  vendor `node_modules` or run `npm install` from `ensure-pi-plugin` (that breaks
  the node-free/offline promise). **Known limitation, stated so the impl task does
  not chase it:** making cache-only Topology B work would require either a
  bundler step (e.g. esbuild) that *still* cannot supply the peer-dep pi SDK, or a
  vendored SDK — both larger than this design. The hermetic guarantee rides on
  Topology A `pi -e`. (`executor_discovery::pi_route_availability` may still
  *report* a `~/.pi/.../pi-worksgood` dir as Topology-B-satisfying; the impl task
  should gate the Node-host transport on `has_node_modules` so that report cannot
  lead to a spawn that dies on an unresolved import.)

This direction never reads or writes `~/.pi`.

**`pi → wg` (Console):** `ensure_pi_plugin(Console)` makes the cache correct
*and* writes the `~/.pi/agent/settings.json` `extensions` entry → a human running
`pi` in the project gets the wg tools/commands auto-loaded, version-locked to the
wg they installed. The trust gate is sidestepped because the entry lives in the
*global* settings file.

### 4.3 `wg pi-plugin` command surface (escape hatch, mirrors `wg skill`)

A new `wg pi-plugin` subcommand group, peer of `wg skill`
(`src/cli.rs` `SkillCommands` / `src/commands/skills.rs`):

| Command | Behavior |
|---|---|
| `wg pi-plugin install` | `ensure_pi_plugin(Console)` — the explicit, blessed install. Mirrors `wg skill install`. |
| `wg pi-plugin status` | print resolved source, cache path, compat version, whether the settings entry is wired, drift verdict. |
| `wg pi-plugin path` | print `ResolvedPlugin.dist_entry` (scriptable). |
| `wg pi-plugin compat-version` | print `WG_PI_PLUGIN_COMPAT_VERSION` (consumed by the plugin's runtime assertion in the pi→wg direction). |

Nobody *needs* to remember `install` — the three wiring points (§4.4) call
`ensure-pi-plugin` automatically — but it exists as the manual repair/verify
handle, exactly as `wg skill install` does.

### 4.4 The three wiring points (same idempotent call at each)

1. **`wg setup` — onboarding.** When a pi route is chosen (the setup route /
   `config_has_pi_route` path), call `ensure_pi_plugin(Console)`. Extend
   `guide_skill_bundle_install` (`src/commands/setup.rs:1958`, today only a
   `claude` arm) with a `pi` arm — interactive prompt when a TTY, auto + summary
   line when headless. Mirrors how setup already nudges `wg skill install`.
2. **`wg profile use pi` — activation.** "I want pi" *is* the declaration; wiring
   is its idempotent side effect. In `use_profile`
   (`src/commands/profile_cmd.rs:720`), after applying the pi profile and right
   alongside the existing endpoint/secret pre-flight loop, call
   `ensure_pi_plugin(Console)` when the activated profile resolves any `pi:`
   route. The daemon hot-reload already happens; the next spawned worker is
   guaranteed a matching plugin. `pi.toml` already *documents* placement
   (`src/profile/templates/pi.toml` "WORKSGOOD PI INSTALL / PLACEMENT") — this makes the
   documentation *executed* instead of read.
3. **JIT at `wg pi-handler` spawn — safety net.** `ensure_pi_plugin(Hermetic)` at
   the top of `pi_handler::run` (§4.2), before topology selection and spawn. This
   closes the last gap: there is **no** path by which a pi worker runs without a
   matching plugin, even if setup/profile steps were skipped. Fast no-op when
   already correct.

These are the *same* primitive at three layers (onboarding → activation →
pre-spawn), so a miss at one layer is caught by the next — defense in depth, no
drift.

---

## 5. Idempotency / self-heal — worked example

```
ensure_pi_plugin(Hermetic):
  source = pick_source()                         # Dev | Cache | EnvOverride
  if source == Cache:
     T = <XDG_CACHE_HOME|~/.cache>/wg/worksgood-pi/<compat>/
     if  exists(T/pi-worksgood/index.js)
     and exists(T/host/wg-pi-host.mjs)
     and read(T/version.json).compat == WG_PI_PLUGIN_COMPAT_VERSION
     and exists(T/.wg-ok):
         pass                                     # already correct → no-op
     else:
         tmp = <…>/worksgood-pi/.<compat>.tmp-<pid>
         extract EMBEDDED_PI_PLUGIN -> tmp; write tmp/version.json; fsync
         touch tmp/.wg-ok; rename(tmp, T)         # atomic repair
     gc_sibling_versions(except = <compat>)       # best-effort
  return ResolvedPlugin{ root: T_or_dev_root, dist_entry, host_script, compat, source, has_node_modules }

ensure_pi_plugin(Console):
  rp = ensure_pi_plugin(Hermetic-core for cache)  # same materialization
  settings = ~/.pi/agent/settings.json
  if settings.extensions does not already point at rp.dist_entry:
      upsert wg-managed entry = rp.dist_entry      # leave user's other entries intact
  return rp
```

Running either twice is a pure no-op the second time. Deleting the cache, the
settings entry, or corrupting the bundle all self-heal on the next call.

---

## 6. Touch-points (real symbols)

- **`worksgood-pi/`** — `src/{index,tools,commands,model-bridge,wg-backend,graph-widget}.ts`,
  `pi-worksgood/` (gitignored dev build, `worksgood-pi/.gitignore`), `host/wg-pi-host.mjs`,
  `package.json` (`@worksgood/pi` `0.1.0`, `pi.extensions: ["./pi-worksgood/index.js"]`,
  peer deps `*`), the `selftest` script + faux-provider `host/`. **New:**
  `worksgood-pi/embedded/` (committed), generated `src/version.ts` (compat stamp).
- **`src/commands/pi_handler.rs`** — `run`, `rpc_spawn_args` (gains `-e`/`-ne`),
  `select_topology` (gains `has_node_modules` gate), `RpcTransport`/`NodeHostTransport`.
- **`src/executor_discovery.rs`** — `pi_route_availability(_in)`, `PiNodeHost`,
  `pi_plugin_candidate_dirs` (`WG_PI_PLUGIN_DIR` override), `locate_pi_node_host`.
- **`src/commands/skills.rs`** — `run_install` (`include_str!`→place) is the
  precedent the new `wg pi-plugin install` mirrors; `is_claude_skill_installed`
  the status-check shape.
- **`src/commands/setup.rs`** — `guide_skill_bundle_install` (add `pi` arm),
  `is_claude_skill_installed` analog.
- **`src/commands/profile_cmd.rs`** — `use_profile` (wiring point #2), the
  endpoint/secret pre-flight loop, `trigger_daemon_reload`.
- **`src/profile/templates/pi.toml`** + `src/profile/named.rs` (`STARTER_PI`) —
  the documented 3 placements; the `pi` profile.
- **`src/agency/mod.rs`** — `WG_AGENCY_COMPAT_VERSION = "1.2.4"`, the
  compat-version precedent for the new `WG_PI_PLUGIN_COMPAT_VERSION`.
- **`src/cli.rs`** — `Commands::PiHandler`, `SkillCommands` (model for the new
  `PiPluginCommands`).
- **`WG_DAEMON_SOCKET`** — the dormant IPC seam `wg-backend.ts` stubs; **stays
  dormant**.
- **pi SDK 0.79.x** (`@earendil-works/pi-coding-agent`) — `--extension/-e`,
  `--no-extensions/-ne`, `pi install/list/remove`, `settings.json`
  `extensions`/`packages`, `~/.pi/agent/settings.json`, jiti loader,
  `DefaultResourceLoader({extensionFactories})`. Verified on `0.79.10`.

---

## 7. Original implementation plan (completed)

> Each step lists its file scope. Steps 1–4 have no behavior change and can land
> first; behavior wiring is 5–8. No two parallelizable steps share a file.

1. **Compat const + module.** Add `src/pi_plugin/mod.rs` with
   `WG_PI_PLUGIN_COMPAT_VERSION` (start `"0.1.0"`). Wire `mod pi_plugin;`.
   *Validation:* `cargo build`; unit test that the const is non-empty + semver-ish.
2. **Embed pipeline.** Add `make embed-worksgood-pi` (+ `xtask`/script): build the
   plugin, copy the curated runtime subset into committed `worksgood-pi/embedded/`,
   stamp `version.json` from the const, generate `worksgood-pi/src/version.ts`. Commit
   the first `worksgood-pi/embedded/`. *Validation:* `make embed-worksgood-pi` is a
   no-op on a clean tree; `version.json.compat == WG_PI_PLUGIN_COMPAT_VERSION`.
3. **Embed into binary + CI gate.** `include_dir!`(or generated slice) of
   `worksgood-pi/embedded/`; add the CI staleness job (rebuild → `git diff
   --exit-code worksgood-pi/embedded`). *Validation:* a Rust test asserts the
   embedded `version.json` parses and equals the const; CI gate green.
4. **`ensure-pi-plugin` core.** New `src/commands/pi_plugin_install.rs` (or under
   `src/pi_plugin/`): `EnsureMode`, `ResolvedPlugin`, `wg_cache_dir()`, source
   selection (§Decision 4), atomic extract + `.wg-ok` + GC (§Decision 3/5), the
   Console settings-entry upsert (§Decision 2). *Validation:* unit tests for
   no-op/repair/atomicity/GC over a temp `XDG_CACHE_HOME` and a temp `HOME`/`.pi`.
5. **`wg pi-plugin` CLI.** Add `PiPluginCommands` to `src/cli.rs` + dispatch in
   `src/main.rs`: `install`/`status`/`path`/`compat-version`. *Validation:*
   `wg pi-plugin install` then `wg pi-plugin status` reports "wired", second run
   is a no-op; `compat-version` prints the const.
6. **Plugin runtime compat assertion.** In `worksgood-pi/src/index.ts`, read
   `EMBEDDED_COMPAT` from generated `version.ts`, compare against
   `WG_PI_PLUGIN_COMPAT_VERSION` env or `wg pi-plugin compat-version`; throw on
   mismatch. *Validation:* extend `host/wg-pi-host.mjs --selftest` to assert a
   forced mismatch surfaces as an `extensionsResult.errors` entry; `npm test`.
7. **Hermetic wiring in `pi-handler`.** Call `ensure_pi_plugin(Hermetic)` in
   `pi_handler::run`; inject `WG_PI_PLUGIN_COMPAT_VERSION` + `WG_PI_PLUGIN_DIR`
   into the child env; add `-e <dist_entry> -ne` to `rpc_spawn_args`; gate
   Topology B on `has_node_modules`. *Validation:* `test_rpc_spawn_args_*` asserts
   `-e <abs>` + `-ne` present and still no `--api-key`; topology test asserts B is
   refused without `node_modules`.
8. **Console wiring at the two declaration points.** `wg setup`
   (`guide_skill_bundle_install` pi arm) and `wg profile use pi`
   (`use_profile`, pi-route branch) call `ensure_pi_plugin(Console)`.
   *Validation:* `wg profile use pi` against a temp `HOME` writes the settings
   `extensions` entry; re-running is a no-op; a smoke scenario under
   `tests/smoke/scenarios/` exercises `profile use pi` → settings entry present →
   `pi -e <path> -ne` selftest loads the plugin.

**Out of scope (explicitly):** activating `WG_DAEMON_SOCKET` IPC; cache-only
Topology B (needs a vendored/bundled pi SDK — §4.2); publishing the plugin to
npm (the embed is the source of truth for wg-spawned pi). These remain follow-ups.
