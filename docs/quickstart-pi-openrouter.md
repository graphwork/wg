# Quickstart: WorksGood + Pi + a free OpenRouter model

> **Verified against:** `wg 0.1.0` (this repo) and
> `@earendil-works/pi-coding-agent` **0.82.0**. Re-check the commands against
> your installed versions — see [Troubleshooting](#troubleshooting) when
> something differs.

This is the canonical, copy-pasteable path from a clean machine to a live
`wg tui` driving a **free** OpenRouter model through **Pi**, at zero cost, so
you can confirm the whole wiring before spending credits. README and the
[graphwork.github.io](https://graphwork.github.io/) site mirror this same path;
if they disagree, **this file is the source of truth**.

---

## 0. The five names (read this once)

| Identity | What it is |
|---|---|
| **WorksGood** | the product / project |
| **`worksgood`** | the installed attended lifecycle concierge (`setup`, `status`, `stop`, `restart`, `tui`) |
| **`wg`** | the installed complete expert task/tool CLI; Pi tools invoke this backend |
| **`@worksgood/pi`** | the npm package name of the WorksGood↔Pi integration. It is **not** installed from npm today; the version-locked build is *embedded in the `wg` binary* and installed by `wg pi-plugin install`. |
| **`pi-worksgood`** | the label Pi shows for the integration (tools, `/wg` commands, `/model` detail). |

**Pi** (`@earendil-works/pi-coding-agent`) is a separate product. WorksGood
uses Pi as its **sole LLM model plane**: Pi owns provider login, model
discovery, endpoints, availability, and reported cost; WorksGood owns the task
graph plus exact per-role `pi:<provider>:<model>` routes. See
[Pi model-plane ownership](pi-model-plane.md).

> **Heads-up on the `wg` name.** `wg` collides with WireGuard's `wg(8)`. If you
> also use WireGuard, install WorksGood's expert `wg` on a private path you
> control. The human-facing `worksgood` command is shipped by default. There is
> deliberately no `worksg` alias, and Pi integrations still call full `wg`
> verbs that the lifecycle concierge does not expose.

---

## 1. Install WorksGood (`worksgood` + `wg` + `nex`)

Requires the Rust toolchain ([rustup](https://rustup.rs/)).

```bash
cargo install --git https://github.com/graphwork/wg --locked
worksgood --help
wg --version
nex --version
```

From a source checkout instead:

```bash
cargo install --path . --locked
```

Make `~/.cargo/bin` reachable:

```bash
# bash/zsh — add once, then: source ~/.bashrc  (or restart the shell)
echo 'export PATH="$HOME/.cargo/bin:$PATH"' >> ~/.bashrc
```

---

## 2. Install Pi

Pi is an npm package. Requires Node.js 20+.

```bash
npm install -g --ignore-scripts @earendil-works/pi-coding-agent
pi --version
```

`--ignore-scripts` disables npm lifecycle scripts; Pi does not need them.

**macOS** — install Node first (`brew install node`, or [nvm](https://github.com/nvm-sh/nvm)),
then the same `npm install -g` line.

**Termux (Android)** — see Pi's [Termux guide](https://pi.dev/docs/latest/termux):

```bash
pkg update && pkg upgrade
pkg install nodejs git           # termux-api optional, for clipboard
npm install -g --ignore-scripts @earendil-works/pi-coding-agent
```

---

## 3. Authenticate Pi with OpenRouter

You only do this **once**, in Pi — WorksGood never sees or stores your
OpenRouter key.

```bash
cd /path/to/any/project
pi                              # start an interactive Pi session
```

Inside Pi, run:

```text
/login openrouter
```

then choose **Sign in with OpenRouter**. This opens OpenRouter's PKCE OAuth
flow in your browser, sign in, and Pi stores a **user-controlled API key**
(minted from your OpenRouter credits) in `~/.pi/agent/auth.json` with `0600`
permissions. Sign out any time with `/logout`.

> **Safe key handling — never put a secret in your shell history.**
> Prefer `/login` (above). If you cannot use OAuth, set the key from a file or
> an interactive prompt instead of typing it on the command line:
> ```bash
> # option A: store it in Pi's auth file (0600), no shell history
> #   run /login inside pi, pick OpenRouter, choose "Use an API key"
>
> # option B: export in your shell rc (~/.profile / ~/.zshrc), never inline
> #   export OPENROUTER_API_KEY="$(cat ~/.config/openrouter/api_key)"
> ```
> **Do not** use `pi --api-key sk-or-...` — that lands the key in your shell
> history and process list. Credential resolution order in Pi is:
> `--api-key` flag → `~/.pi/agent/auth.json` → environment variable →
> `~/.pi/agent/models.json`. See Pi's
> [Providers](https://pi.dev/docs/latest/providers) reference.

You need a small amount of OpenRouter credit even for `:free` models in some
accounts; free models also have per-day/per-minute request caps. Check your
allowance at <https://openrouter.ai/keys>.

---

## 4. Find a currently-available free model

Free OpenRouter models are tagged with a `:free` suffix. **Availability,
context windows, rate limits, and tool support change frequently** — discover
them at run time, never hard-code one.

```bash
pi --list-models ":free"
```

A credential-free listing of every currently-free OpenRouter model Pi knows:

```text
provider    model                                               context  max-out  thinking  images
openrouter  google/gemma-4-31b-it:free                          262.1K   32.8K    yes       yes
openrouter  nvidia/nemotron-3-ultra-550b-a55b:free              1M       65.5K    yes       no
openrouter  openai/gpt-oss-20b:free                             131.1K   32.8K    yes       no
...                                                  (the list rotates over time)
```

Filter further, e.g. only OpenRouter models, or by name:

```bash
pi --list-models "openrouter/"          # all OpenRouter models (free + paid)
pi --list-models "openrouter/llama"     # fuzzy match
```

> The example `nvidia/nemotron-3-ultra-550b-a55b:free` is **a dated snapshot**,
> not an eternal default. Always re-run `pi --list-models ":free"` and read the
> model card at <https://openrouter.ai/models> for current limits, context
> size, vision support, and **tool/function-call support** (some free models
> do not support tools, which affects agent dispatch).

**Validate the model actually works in Pi** (this needs the auth from step 3):

```bash
pi --model "openrouter/nvidia/nemotron-3-ultra-550b-a55b:free" -p "Reply with the single word OK"
```

A 401 means re-do `/login`; a 404 means the model id is wrong or withdrawn —
re-list and pick another. This is the exact point that requires interactive
OpenRouter login; everything before it is credential-free.

---

## 5. Install the WorksGood Pi integration (`pi-worksgood`)

One command materializes the version-locked build embedded in your `wg` binary
and wires it into Pi's global settings:

```bash
wg pi-plugin install
```

Expected output:

```text
Installed pi-worksgood (npm: @worksgood/pi, compat 0.2.0) from embedded → versioned cache.
  extension: ~/.cache/wg/worksgood-pi/0.2.0/pi-worksgood/index.js
  wired into pi settings: ~/.pi/agent/settings.json
A human `pi` session in this project will now auto-load the wg tools + /wg commands.
```

Verify:

```bash
wg pi-plugin status        # source, cache path, compat version, console wired: yes
wg pi-plugin path          # scriptable: prints the resolved index.js path
wg pi-plugin compat-version   # the WG_PI_PLUGIN_COMPAT_VERSION the plugin asserts
```

A human-run `pi` session now auto-loads the `pi-worksgood` tools
(`wg_ready`, `wg_show`, `wg_add`, `wg_done`, …) and `/wg` commands.

**Two important properties:**

- **Idempotent + self-healing.** Run `wg pi-plugin install` again any time;
  it no-ops when already correct and repairs if the cache or settings drift.
- **Hermetic for WG-spawned workers.** When WorksGood spawns a Pi *worker*
  (`wg pi-handler`), it loads **exactly** the embedded build by absolute path
  (`pi --mode rpc -e <cache>/pi-worksgood/index.js -ne`) and disables all
  discovery. That direction never reads or writes `~/.pi`. Only the *human
  console* direction (you running `pi`) uses the global settings entry written
  here. The compat version (`0.2.0`) is asserted at load and fails loudly on
  mismatch.

**Legacy migration.** If you previously installed the old
`@worksgood/wg-pi-plugin`, `wg pi-plugin install` keeps the legacy record
inert (so your console keeps working offline), points Pi at the one compatible
`pi-worksgood/index.js`, and prints a one-time `pi remove npm:@worksgood/wg-pi-plugin`
command. Run that when convenient.

---

## 6. (Optional) Give Pi web capabilities

Two separate official Pi packages — they are **not** the same thing:

| Package | What it adds | Install |
|---|---|---|
| **`pi-web-access`** | web **search**, URL/page **fetching**, GitHub repo cloning, PDF extraction, YouTube & local video understanding | `pi install npm:pi-web-access` |
| **`pi-agent-browser-native`** | **interactive browser automation** (open pages, click, fill, screenshot, drive real web apps via agent-browser) | `pi install npm:pi-agent-browser-native` |

```bash
pi install npm:pi-web-access               # search + fetch + clone + video
pi install npm:pi-agent-browser-native     # real browser driving
pi list                                    # confirm what's installed
```

The difference: **web search/content access** is read-only research (fetch a
doc, search the web, clone a repo); **browser automation** actively drives a
real browser (log in, click, submit). WorksGood agents can use either when Pi
is the model plane. These are independent of the WorksGood Pi integration
itself. Remove with `pi remove npm:<name>`.

---

## 7. Initialize a WorksGood project

```bash
cd /path/to/your/project
wg init
```

Creates `.wg/` (task graph as JSONL, config, agency, service state). A fresh
project is **graph-only** — it has no LLM route and needs no credentials. You
can open the TUI at this point, but dispatching LLM work requires step 8.

> **Existing users:** the commands below **select** a profile/route; they do
> not delete or overwrite profiles you already configured. Project-scoped
> selection (`wg profile select`) leaves your global `~/.wg/config.toml` and
> `active-profile` untouched.

---

## 8. Select the Pi route with your free model

Pick **one** of these equivalent paths. For a zero-cost smoke test, set **both**
the strong (worker) and weak (agency one-shots) tiers to the free model.

### Path A — project-scoped (recommended; does not change global state)

```bash
wg profile init-starters                                              # writes the `pi` starter profile
wg profile pi --strong "pi:openrouter/nvidia/nemotron-3-ultra-550b-a55b:free" \
                  --weak   "pi:openrouter/nvidia/nemotron-3-ultra-550b-a55b:free"
wg profile select pi                                                   # this project only
```

`wg profile select pi` is project-scoped and explicitly does **not** touch
`~/.wg/config.toml` or `active-profile`.

### Path B — global (make Pi your default everywhere)

Same first two lines, then activate globally:

```bash
wg profile use pi     # rewrites ~/.wg/config.toml, ensures the plugin, hot-reloads the daemon
```

### Path C — one explicit route (writes global config directly)

```bash
wg setup --route pi --yes --model "pi:openrouter/nvidia/nemotron-3-ultra-550b-a55b:free"
```

Note: Path C sets the standard/premium (worker) tiers to your model but leaves
the weak/agency tier on `pi:openrouter:deepseek/deepseek-chat` (paid). Use Path
A or B for a fully free smoke test. Preview any path first with `--dry-run`:

```bash
wg setup --route pi --model "pi:openrouter/nvidia/nemotron-3-ultra-550b-a55b:free" --dry-run
```

Replace the model id with whatever `pi --list-models ":free"` currently returns
and you validated in step 4.

### Verify the effective policy

```bash
wg config --models
```

Every displayed LLM role must show handler `pi`, an exact
`pi:openrouter/<model>:free` route, and a visible effective reasoning level. A
missing route, a non-Pi route, or omitted reasoning **fails closed** — WorksGood
will not silently fall back to another handler.

> **Opening the TUI never selects a route.** `wg tui` (and `wg` with no
> subcommand) is graph-only and non-mutating: it reads the graph/config and
> persists ordinary UI state, but it will not initialize a graph, install
> packages, authenticate, pick a model, or start/reload the service. You must
> explicitly select Pi as above.

---

## 9. Start the service and open the TUI

```bash
wg service start        # start the coordinator/dispatcher
wg service status       # health, daemon PID, dispatcher state
wg tui                  # the operating surface
```

Add your first task, then explicitly release it for dispatch:

```bash
wg add "Smoke test: print hello from a free model" --exec-mode shell
wg publish <task-id> --only
```

Watch the graph evolve in the TUI. Restart after config/package changes:

```bash
wg service reload       # pick up config.toml edits without a full restart
wg service stop         # full stop
wg service start
```

---

## Troubleshooting

**`wg: command not found` / `pi: command not found`**
`~/.cargo/bin` (wg) or your npm global bin (pi) is not on `PATH`. Add them:
```bash
echo 'export PATH="$HOME/.cargo/bin:$(npm config get prefix)/bin:$PATH"' >> ~/.bashrc
source ~/.bashrc
```
Confirm the right binary wins — WorksGood's `wg` can be shadowed by
WireGuard's `wg(8)`. Check `command -v wg` and, if needed, put WorksGood first
or call it by absolute path.

**`Failed to run wg: No such file or directory (os error 2)` (from the TUI)**
A long-running installed `wg tui` could not re-exec itself after the binary was
replaced (e.g. a `cargo install` while the TUI was open). WorksGood now
self-execs via `/proc/self/exe` on Linux, so the fix is to **start the TUI from
the installed binary** and restart it after reinstalling:
```bash
wg service stop 2>/dev/null; pkill -f 'wg tui' 2>/dev/null
cargo install --git https://github.com/graphwork/wg     # or --path . --locked
wg tui
```
See [`docs/bugs/tui-pi-chat-launch-enoent.md`](bugs/tui-pi-chat-launch-enoent.md).

**`role=default has no explicit Pi route` / `WG-PI-ROUTE-MISSING`**
You opened/queried a project before selecting Pi. Run step 8 (select a profile
or run `wg setup --route pi ...`), then re-check with `wg config --models`.

**`compatible plugin build missing` / `pi-worksgood` not loaded**
Run `wg pi-plugin install`, then `wg pi-plugin status` (expect `console wired:
yes`). The build is embedded in the `wg` binary, so reinstalling `wg` and
re-running `wg pi-plugin install` always repairs it.

**Pi compat mismatch (`extension=X wg=Y`)**
The `wg` binary and the installed plugin build disagree. Fix:
```bash
wg pi-plugin install        # re-materializes the build matching this wg
```

**401 / unauthorized from the model**
Pi's OpenRouter auth is missing or stale. In a `pi` session run
`/login openrouter` (or `/logout` then `/login`). WorksGood does not handle
provider credentials — all auth lives in Pi's `~/.pi/agent/auth.json`.

**404 / model not available**
The free model id is wrong or was withdrawn. Re-run
`pi --list-models ":free"`, pick a current one, re-validate with
`pi --model ... -p "..."`, then update your route (`wg profile pi --strong ...
--weak ...` or `wg setup --route pi --model ...`).

**Config changes not taking effect**
```bash
wg service reload          # or stop+start for a full restart
wg config lint             # read-only check for stale/legacy keys
wg migrate config --dry-run   # preview legacy-key rewrites
```

**Old non-Pi config lingering**
Legacy provider/endpoint/model-registry fields are read-only (migration only)
and never authorize dispatch. Inspect and rewrite them:
```bash
wg config lint
wg migrate config --all
```

---

## Concierge and expert boundary

The installed `worksgood` concierge provides the attended lifecycle path and
can select/reconcile a prepared profile before opening the TUI. The explicit
commands above remain the auditable expert path under `wg`. `worksgood` is not
a CLI rename and intentionally does not expose task/tool verbs; there is no
`worksg` alias. Bare `wg` and `wg tui` remain non-mutating, and `pi-worksgood`
continues to invoke `wg` for its complete backend contract.

---

## Reference

- [Pi model-plane ownership](pi-model-plane.md) — the WorksGood/Pi contract.
- [Pi Providers](https://pi.dev/docs/latest/providers) — all auth methods and env vars.
- [Pi Packages](https://pi.dev/packages) — `pi install` / extension model.
- [`@worksgood/pi` README](../worksgood-pi/README.md) — what the integration registers.
- [Pi plugin install design](design-pi-plugin-install.md) — hermetic vs console, compat handshake.
- OpenRouter models: <https://openrouter.ai/models> · keys: <https://openrouter.ai/keys>.
