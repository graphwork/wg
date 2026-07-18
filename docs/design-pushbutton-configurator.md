# Design: push-button WorksGood configurator

**Task:** `study-pushbutton-worksgood`

**Status:** design/research only — **no command or name is approved for implementation**

**Evidence date:** 2026-07-18

**WG under test:** `wg 0.1.0` from this checkout

**Pi under test:** `@earendil-works/pi-coding-agent` 0.80.10

## Decision summary

1. **The executable name is an explicit unresolved release gate.** The original draft recommended `wg onboard`; the WireGuard audit below invalidates making that recommendation unconditionally. WireGuard has owned the cross-platform `wg(8)` command for years, and installing WorksGood's `wg` earlier on `PATH` can shadow an administrative network tool. Maintainers must choose among three outcomes after the inventory and staged-migration gates: **A)** keep `wg` with a collision guard, **B)** make `worksg` the full canonical CLI and offer a verified `wg` alias only when safe, or **C)** add a `worksg` concierge while keeping the full CLI canonical at `wg`.
2. **No renaming or dual-bin release is approved.** Outcome B best eliminates the permanent namespace collision but has high transition risk: the repository contains thousands of CLI mentions, stored executable strings, Pi's Node backend shells out to literal `wg`, service assets and scripts assume the name, and two installed copies can skew. Outcome A has the least migration risk but cannot make future `wireguard-tools` installs or PATH reordering safe. Outcome C improves first-run discoverability but does not solve the collision after onboarding. The document remains a go/no-go study, not a rename plan.
3. **The configurator must be an explicitly mutating operation.** Independently of the binary decision, the stable verb is `onboard`: either `wg onboard` (A/C) or `worksg onboard` (B). Bare `wg` stays non-mutating because WireGuard's bare `wg` means `wg show`; bare `worksg` could be an attended concierge only under B, but help-plus-`onboard` is more scriptable and is the safer default pending UX testing.
4. **Keep the graph TUI setup-neutral.** `wg tui` today—and `worksg tui` under B—may read graph/config and persist ordinary UI state, but must not initialize a graph, select/replace an execution route, install packages, authenticate, or start/reload a service.
5. **Never overwrite or divert WireGuard.** Installers and package definitions must inspect destination and PATH identities without executing a foreign `wg`. A system/package-manager-owned `wg`, an unknown `wg`, or any non-byte-identical candidate means “no compatibility alias” unless the user chooses a separate private path. `--force` is not permission to replace WireGuard.
6. **`worksg` is a candidate, not cleared property.** Exact npm lookup returned 404 and no local executable was found on 2026-07-18, but a GitHub account already owns `worksg`; registry absence is not trademark, package-manager, domain, or PATH clearance. `wsg` is shorter but npm already has `wsg@0.0.1` and the acronym has many unrelated uses. `worksgood` is clearer but long and was rejected by the requester as a daily command.
7. **Do not promise `pi-worksgood` as an npm install today.** `npm view pi-worksgood` and `npm view @worksgood/pi` both returned 404. The shipped, version-locked WorksGood Pi integration is embedded in the verified WorksGood binary and currently installed with `wg pi-plugin install`. A future registry publication is a separate publishing/security decision.
8. **Discover free OpenRouter models at run time; never bake one into onboarding.** Filter the public catalog, then test the exact candidate through Pi with Pi-owned authentication. Persist only a user-confirmed, successful route. Never switch to nex, Claude, Codex, another provider, or OpenRouter's random free router after a failure unless the user separately and explicitly authorizes a same-system fallback under WG's existing execution-selection contract.
9. **Installation trust starts before the concierge exists.** Prefer package managers or downloaded, inspected, checksum/attestation-verified release artifacts. An installed concierge may install/verify Pi after confirmation; it cannot securely bootstrap an absent WorksGood binary by magic.
10. **`--yes` never means “choose for me.”** Noninteractive use requires every consequential choice. It never drives `/login`, accepts package code, switches an existing non-Pi profile, opens a TUI, silently chooses a model, or installs a colliding `wg` alias.

## Non-negotiable invariants

- A fresh WG remains graph-only until the user confirms a handler-first route, as required by [`docs/design-explicit-execution-system.md`](design-explicit-execution-system.md).
- Detection produces annotations, not authority. Finding `pi`, a key, an auth file, an active profile, or a free model does not select it.
- The final routing confirmation names the handler, wire/provider, exact model, scope, config source, auth owner, plugin source/version, and service action.
- Secrets never appear in argv, command logs, transaction journals, shell history, environment dumps, dry-run output, or error text.
- `<canonical> onboard --dry-run` writes nothing, including usage history and transaction state.
- Resume is based on observed state plus a journal, not on blindly trusting a previous checkpoint.
- Rollback removes only objects created by the transaction and restores backed-up files atomically. It never deletes a pre-existing graph, profile, daemon, package, or credential.
- Entering the TUI is a post-commit, explicit attended action.

## Source-verified current behavior

### WG

| Surface | Verified behavior | Evidence |
|---|---|---|
| Bare `wg` | No subcommand prints help and returns before usage logging. | `src/main.rs:745-768` |
| `wg init` | With no model/route/executor it creates a graph-only project; `--dry-run` does not create it. It refuses an existing `.wg`/legacy sibling rather than merging. | `src/commands/init.rs:50-108`, `:111-137`, `:230-268` |
| `wg setup` | Interactive setup offers **“Not now — keep this WG graph-only”** as the first route and only preselects an existing explicit model. Noninteractive setup requires a route or handler-first model. | `src/commands/setup.rs:1667-1741`, `:1249-1282` |
| Setup writes | Route setup previews a diff, backs up an existing target, then writes global/local/both. | `src/commands/setup.rs:1455-1576` |
| Pi setup split | Interactive Pi setup explains that Pi owns `pi:` auth while WG-owned native OpenRouter traffic is separate. | `src/commands/setup.rs:2325-2416` |
| Core Pi plugin | `wg pi-plugin install` materializes the WG-compatible embedded build and wires Pi settings; status/path/compat are separate. | `src/commands/pi_plugin_install.rs:1-94`; [`docs/design-pi-plugin-install.md`](design-pi-plugin-install.md) |
| Profile activation | `wg profile use` overlays the global config, backs up/clears local routing overrides, records the active profile, ensures the Pi plugin for Pi routes, and hot-reloads a running daemon unless `--no-reload`. | `src/commands/profile_cmd.rs:741-865` |
| Service idempotence | `wg service start` preflights explicit selection before forking, reuses a live service by returning success, cleans stale state, detects orphan daemons, and requires `--force` before killing/replacing one. | `src/commands/service/mod.rs:1133-1271` |
| Service persistence | The spawned daemon gets null stdio and `setsid()` on Unix, so closing a mosh/SSH PTY does not intentionally kill it. | `src/commands/service/mod.rs:1281-1364` |
| TUI | `wg tui` directly runs the viewer; it does not call setup, init, or service start. | `src/main.rs:3399-3435` |
| Installer | The shell installer supports version/channel/path/dry-run, checks SHA256, optionally verifies GitHub attestations, installs atomically, and writes `~/.wg/install-receipt.toml`. | `scripts/install-wg.sh:20-42`, `:250-345`, `:376-418`, `:474-570`; [`docs/guides/install.md`](guides/install.md) |

Two current inconsistencies matter to this design:

- `wg setup --route pi --scope local --yes` does **not** run the interactive post-save plugin prompt. The declarative plugin guarantee exists in interactive setup, `wg profile use pi`, and JIT Pi worker spawn, but the headless setup route by itself leaves the human Pi console unwired.
- The current Pi route template still emits deprecated `agent.executor`/`dispatcher.executor` keys and bare `openrouter:` weak-tier entries. It also contains `pi:openrouter/vendor/model` strings while the ratified explicit-execution design and setup validation example specify handler-first `pi:openrouter:vendor/model`. A fresh prototype immediately produced migration warnings. Onboarding must first settle/write one canonical Pi dialect and must not call a warning-filled config “ready.”

### Pi 0.80.10

The installed package and upstream checkout are both version 0.80.10 at commit [`3da591ab74ab9ab407e72ed882600b2c851fae21`](https://github.com/earendil-works/pi-mono/tree/3da591ab74ab9ab407e72ed882600b2c851fae21).

| Surface | Verified behavior | Evidence |
|---|---|---|
| Install | Official docs prefer `npm install -g --ignore-scripts @earendil-works/pi-coding-agent`; uninstall ownership stays with that package manager. | [quickstart lines 5-33](https://github.com/earendil-works/pi-mono/blob/3da591ab74ab9ab407e72ed882600b2c851fae21/packages/coding-agent/docs/quickstart.md#L5-L33) |
| Auth | `/login` is an interactive command. API-key providers, including OpenRouter, can store credentials in `~/.pi/agent/auth.json`; Pi creates it with mode 0600 and stored auth precedes environment auth. | [quickstart lines 42-67](https://github.com/earendil-works/pi-mono/blob/3da591ab74ab9ab407e72ed882600b2c851fae21/packages/coding-agent/docs/quickstart.md#L42-L67), [providers lines 17-25 and 52-165](https://github.com/earendil-works/pi-mono/blob/3da591ab74ab9ab407e72ed882600b2c851fae21/packages/coding-agent/docs/providers.md#L17-L165), [interactive command dispatch](https://github.com/earendil-works/pi-mono/blob/3da591ab74ab9ab407e72ed882600b2c851fae21/packages/coding-agent/src/modes/interactive/interactive-mode.ts#L2703-L2710) |
| Auth ownership | `/logout` removes saved login credentials; environment variables and model config remain. There is no documented headless `pi login` command. | [providers lines 17-25](https://github.com/earendil-works/pi-mono/blob/3da591ab74ab9ab407e72ed882600b2c851fae21/packages/coding-agent/docs/providers.md#L17-L25), `packages/coding-agent/src/modes/interactive/interactive-mode.ts:4994` |
| Models | `/model`/Ctrl+L select models; `--list-models` lists only available/authenticated models. Catalogs may refresh and cache in `models-store.json`; `pi update --models` forces refresh. | [providers lines 1-4](https://github.com/earendil-works/pi-mono/blob/3da591ab74ab9ab407e72ed882600b2c851fae21/packages/coding-agent/docs/providers.md#L1-L4), `packages/coding-agent/src/package-manager-cli.ts:154-170` |
| Packages | `pi install npm:<package>@<version>` and `pi remove` update global settings by default; `-l` targets `.pi/settings.json`. Pinned npm versions are skipped by package updates. | [packages lines 12-67](https://github.com/earendil-works/pi-mono/blob/3da591ab74ab9ab407e72ed882600b2c851fae21/packages/coding-agent/docs/packages.md#L12-L67) |
| Package trust | Pi packages/extensions execute with the user's full permissions; source review is required. Project-local packages load only after project trust. | [packages lines 9-12](https://github.com/earendil-works/pi-mono/blob/3da591ab74ab9ab407e72ed882600b2c851fae21/packages/coding-agent/docs/packages.md#L9-L12), [security](https://github.com/earendil-works/pi-mono/blob/3da591ab74ab9ab407e72ed882600b2c851fae21/packages/coding-agent/docs/security.md) |
| Offline | `--offline`/`PI_OFFLINE=1` disables Pi startup network work, including update/package checks and telemetry. It cannot make an uncached provider/model usable. | `packages/coding-agent/docs/settings.md:79-81` |
| Termux | Pi documents Termux installation with `nodejs`, `termux-api`, and git; image clipboard/native optional dependencies are limited. | [Termux guide](https://github.com/earendil-works/pi-mono/blob/3da591ab74ab9ab407e72ed882600b2c851fae21/packages/coding-agent/docs/termux.md) |
| tmux | Pi recommends tmux 3.5+ with `extended-keys on` and `extended-keys-format csi-u`; older 3.2-3.4 can use xterm modified-key encoding. | [tmux guide](https://github.com/earendil-works/pi-mono/blob/3da591ab74ab9ab407e72ed882600b2c851fae21/packages/coding-agent/docs/tmux.md) |

The live `https://pi.dev/install.sh` fetched on 2026-07-18 is an interactive npm installer. It requires Node 22.19+, can install Node, uses `npm install -g --ignore-scripts`, and has an experimental locked-install path. The bootstrap script itself is still remote code when piped directly into `sh`; onboarding should therefore recommend a package-manager command or inspect-first download, not repeat the pipe-to-shell promise.

### OpenRouter discovery snapshot

Official OpenRouter documentation exposes `GET https://openrouter.ai/api/v1/models` and supports `supported_parameters=tools`; model records include `id`, `context_length`, `pricing`, `supported_parameters`, and expiration metadata. `:free` variants are volatile and may have lower availability/rate limits. The `openrouter/free` router chooses a free model dynamically; that is useful for casual chat but is **not** an auditable pinned WG execution route.

A credential-free query on 2026-07-18, filtered to text input, at least 65,536 context tokens, `tools`, and zero prompt/completion price, returned 15 candidates. Examples included:

- `tencent/hy3:free` — 262,144 context, expiration 2026-07-21;
- `poolside/laguna-xs-2.1:free` — 262,144 context;
- `cohere/north-mini-code:free` — 256,000 context;
- `nvidia/nemotron-3-super-120b-a12b:free` — 1,000,000 context;
- `qwen/qwen3-coder:free` — 1,048,576 context, expiration 2026-07-19.

This snapshot proves why a baked default is wrong: two plausible coding candidates expired the next day. Official sources: [models API](https://openrouter.ai/docs/api/api-reference/models/get-models), [free variants](https://openrouter.ai/docs/guides/routing/model-variants/free), [free router](https://openrouter.ai/docs/guides/routing/routers/free-router), and [tool calling](https://openrouter.ai/docs/guides/features/tool-calling).

## Credential-free disposable prototype

No production config was touched. The flow ran under:

```text
HOME=/tmp/wg-pushbutton-prototype.oxtivI/home
XDG_CACHE_HOME=/tmp/wg-pushbutton-prototype.oxtivI/cache
project=/tmp/wg-pushbutton-prototype.oxtivI/project
```

with inherited WG/key variables removed. Commands and observations:

| Command | Result / friction |
|---|---|
| `wg` | Printed help, exit 0, created no WG/Pi file. This is the right bare-command contract. |
| `wg setup --route pi --scope local --dry-run` | Wrote nothing, but printed a large full-config serialization after the delta and proposed a paid hard-coded GLM route plus native OpenRouter weak route. |
| `wg init --no-agency` | Created graph-only `.wg`, `.gitignore`, executor examples, `AGENTS.md`, and `CLAUDE.md`; clearly said no execution system was selected. |
| `wg setup --route pi --scope local --yes` | Selected Pi but did not install the console plugin. The written route immediately caused deprecated-executor and bare-provider warnings on `wg status`. |
| `wg pi-plugin status`; `wg pi-plugin install` twice | Status correctly identified missing console wiring. Install was successful; the second run left `settings.json` byte-identical. Because this was a checkout build it used live `worksgood-pi/pi-worksgood`; a release build uses the embedded cache. |
| `pi --offline --list-models openrouter` | Exit 0 but “No models available. Use /login…”. This confirms discovery alone is not readiness and Pi auth cannot be skipped. Pi also created an empty/auth scaffold under its own config root. |
| `pi list --no-approve` | “No packages installed.” The WorksGood integration is a settings extension, not a Pi-managed package; this is expected but can confuse users. |
| `wg service start --no-chat-agent` twice | First start detached one daemon; second reported “already running” and did not duplicate it. `status` and `stop` worked. Auth/model generation was not proven by service start. |
| `wg tui --recording` in a disposable tmux session | Opened an empty graph and exited without starting a daemon or changing the file set. Existing config warnings flooded the small terminal, showing why onboarding must finish lint-clean before TUI launch. |

Missing primitives exposed by the prototype:

1. No credential-safe, noninteractive Pi auth-status command.
2. No Pi “probe this model with a forced tool call” command.
3. No WG command that discovers **free + tool-capable + context-qualified** OpenRouter models and tests them through Pi-owned auth.
4. Headless `wg setup --route pi` does not guarantee human-console plugin wiring.
5. Current Pi setup defaults produce config warnings.
6. No single command plans, journals, resumes, and rolls back install/auth/plugin/profile/graph/service work.
7. `pi list` cannot explain a WG-managed settings extension; onboarding must report both `wg pi-plugin status` and `pi list` separately.

## The `wg` / WireGuard collision: prevalence and impact

This is not a hypothetical registry-name conflict. WireGuard's official userspace package installs **`wg`**, documented as “set and retrieve configuration of WireGuard interfaces.” Bare WireGuard `wg` defaults to `show`, so changing what bare `wg` executes can hide or disclose network state rather than merely produce a command-not-found error. Its source identifies `--version` as `wireguard-tools v…`, but the migration detector must not execute a foreign candidate merely to identify it. Official evidence: [installation matrix](https://www.wireguard.com/install/), [quick start](https://www.wireguard.com/quickstart/), [`wg.c`](https://github.com/WireGuard/wireguard-tools/blob/master/src/wg.c), and [wireguard-tools](https://github.com/WireGuard/wireguard-tools/).

The Linux kernel gained WireGuard in 5.6, but the `wg` userspace binary remains a separate, common package. No defensible install-rate statistic was found, so this study does **not** invent a prevalence percentage. Package availability, standard paths, and the privilege/sensitivity of the command are sufficient to treat collision as high-impact.

### Platform/package collision matrix

| Environment | WireGuard packaging / `wg` location | WorksGood collision behavior | Risk / required policy |
|---|---|---|---|
| Debian / Ubuntu | `wireguard-tools` (or the `wireguard` metapackage) owns `/usr/bin/wg`, `/usr/bin/wg-quick`, `wg(8)`, systemd units, and bash completion. [Debian file list](https://packages.debian.org/bookworm/amd64/wireguard-tools/filelist) | A WorksGood deb cannot also own `/usr/bin/wg`; [Debian policy](https://www.debian.org/doc/debian-policy/ch-files.html) says different programs must not install the same filename even in different default-PATH directories. A user install in `~/.local/bin` may silently shadow `/usr/bin/wg`. | Never `Conflicts`/`Replaces`/`dpkg-divert` WireGuard. A distro WorksGood package must use `worksg` only; user installer may add `wg` only after a positive same-binary identity check and opt-in. |
| Fedora / RHEL / Rocky / SUSE | `wireguard-tools` provides `wg`/`wg-quick`; RHEL documents `dnf install wireguard-tools` and then `wg`. | RPM reports a file conflict if two packages own differing `/usr/bin/wg`; forcing replacement would damage WireGuard. A Cargo/user path can still shadow it. | Same rule as Debian; never `--replacefiles` or claim `/usr/bin/wg` from a system package. |
| Arch | `pacman -S wireguard-tools`; package file list contains `/usr/bin/wg`. | Package-owned path conflict or user-PATH shadowing. | Canonical `worksg` is co-installable; `wg` alias cannot be a default package file. |
| Alpine / containers | `apk add wireguard-tools`; `wg` is used in small network/admin images. | Containers often have a short PATH and copy a WorksGood binary directly to `/usr/local/bin/wg`, which typically precedes `/usr/bin`. | Container images must use `/usr/local/bin/worksg`; do not mask the networking tool. Existing benchmark Docker recipes are migration inventory. |
| Homebrew (macOS and Linuxbrew) | `brew install wireguard-tools`; formula builds `wg` and `wg-quick` into the Homebrew prefix. | Two formulae linking `bin/wg` produce a `brew link` conflict; a Cargo `~/.cargo/bin/wg` may win or lose depending user PATH. Homebrew itself does not make cross-manager precedence deterministic. | A WorksGood formula must link `worksg`, not `wg`; an optional alias is a user-managed private link with a caveat, not a formula conflict with `wireguard-tools`. |
| MacPorts / FreeBSD pkg | MacPorts `wireguard-tools` and FreeBSD `pkg install wireguard-tools` install `wg` under their prefixes (commonly `/opt/local/bin` or `/usr/local/bin`). | Same-prefix file conflict or PATH-dependent shadowing. | Default package contains `worksg` only. |
| Nix / NixOS / Home Manager | `pkgs.wireguard-tools` installs `bin/wg`; profile/build environments normally reject same-priority file collisions. Store paths themselves coexist, but a composed profile cannot expose both as `wg` without priorities/renaming. | Adding a WorksGood derivation with `bin/wg` can make the profile build fail or require a priority that silently chooses one. | Default Nix output exposes `worksg`; do not solve with `ignoreCollisions` or priority. An opt-in alias belongs in a user wrapper/environment that excludes WireGuard and is visibly non-portable. |
| Cargo install | Current crate `worksgood` has `[[bin]] wg` and `[[bin]] nex`; Cargo installs all bins into one root by default. It refuses a differing `wg` already **in that root** unless `--force`, but does not inspect same-named commands elsewhere on PATH. | `cargo install --force` can overwrite a foreign `$CARGO_HOME/bin/wg`. If `/usr/bin/wg` or Homebrew `wg` is elsewhere, Cargo succeeds and shell order chooses one silently. | Never instruct `--force` until the exact destination is proven WorksGood-owned. If B is chosen, publish `worksg` as default and gate `wg` behind a Cargo feature/separate alias artifact—or Cargo still attempts both and can make the whole install fail. |
| HPC / environment modules | Exact prevalence is unknown. Login and compute-node PATHs differ; modules can prepend/remove bins. WireGuard is used in cluster networking literature, but many centers restrict tunnel administration. | A name may resolve to WorksGood on a login node, WireGuard on a compute node, or change after `module load`. Stored `wg …` task commands are therefore non-portable even when installation initially looked safe. | Require a canonical absolute/verified WorksGood path for daemon/plugin/generated jobs; do not rely on interactive-shell PATH. Test login/compute/module contexts separately. |
| Termux | Official Termux has a `wireguard-tools` package; its environment intentionally centers `$PREFIX/bin`, so packages share one command namespace. | There is no `/usr` versus user-local safety boundary inside the normal Termux prefix. A second `wg` package cannot safely coexist. | If WG gains supported Android builds, use `worksg` only. Never replace `$PREFIX/bin/wg`. |
| Windows | The official WireGuard installer ships `C:\Program Files\WireGuard\wg.exe` and can add its directory to PATH. | WorksGood currently releases `wg.exe`; PATH order and installer directory decide which runs. Copying over the WireGuard file is unacceptable. | Canonical `worksg.exe`; optional `wg.exe` only in a separate WorksGood directory after PATH audit and explicit opt-in. |

### Future-install and uninstall behavior

An install-time “`wg` absent” check is necessary but not sufficient:

- **WorksGood first, WireGuard later:** a package manager may reject WireGuard because the same managed prefix already owns `wg`; or it may install `/usr/bin/wg` while `~/.cargo/bin/wg` keeps shadowing it. The administrator can believe WireGuard is installed while `sudo wg …` and `wg …` resolve differently because secure/sudo PATH differs.
- **WireGuard first, WorksGood later:** the current release installer chooses a user bin directory and unconditionally atomically replaces `<install-dir>/wg`; it does not inspect every PATH candidate or package ownership. Cargo only checks its destination root. Both can create a shadow without touching `/usr/bin`.
- **PATH changes later:** shell initialization, Homebrew, rustup/Cargo, environment modules, sudo, cron, systemd, tmux servers, mosh, and IDEs can all observe a different winner.
- **WorksGood uninstall:** must remove only a receipt-matched WorksGood `worksg` and an alias whose symlink target/inode/hash still matches that receipt. It must never `rm $(command -v wg)` and never remove a package-manager-owned WireGuard file. If an old WorksGood `wg` is left because it drifted, report it for manual review.
- **WireGuard uninstall:** cannot be used as permission to install an alias automatically. The next WorksGood upgrade may offer the alias again, but only with explicit opt-in; an existing shell hash table should be refreshed (`hash -r`/new shell).

Outcome A therefore reduces installer accidents but cannot close the lifetime collision. Outcome B closes the daily-command collision once internal and external callers migrate. Outcome C does not close it.

## Non-executing PATH identity protocol

Never run an unknown `wg`, including `wg --version`, during install/detection. WireGuard's `--version` is currently read-only, but another executable or shell function named `wg` need not be. Bare `wg` is especially unsafe because official WireGuard treats it as `show`.

Given the already verified WorksGood executable `self` and each command candidate from every PATH element (do not use only `command -v`):

1. Parse PATH as data; reject empty/relative/world-writable elements for alias decisions. Use filesystem APIs, not a shell, and include Windows `PATHEXT` handling.
2. `lstat` then canonicalize each candidate without following an unbounded/surprising symlink chain. Record device/inode, owner, mode, and canonical path. Do not execute it.
3. A candidate is **WorksGoodSameBuild** only if one of these holds:
   - it is the same device+inode as the running verified binary;
   - its canonical symlink target is that binary inside the receipt-owned install root; or
   - its SHA-256 exactly equals a signed release-manifest hash/receipt for the same WorksGood version and target.
4. Query package ownership by exact path as secondary classification (`dpkg-query -S`, `rpm -qf`, `pacman -Qo`, `apk info -W`, `brew list`, `pkg which`, Nix store metadata, Windows Authenticode/product metadata). Querying the trusted manager is allowed; never execute the candidate. Ownership by `wireguard-tools`/WireGuard is **WireGuardOwned**. Ownership by another/unknown package is **ForeignOwned**.
5. An unowned candidate whose bytes do not match the verified WorksGood manifest is **Unknown**, not “probably old WG.” Do not replace it. Old WorksGood versions can be recognized only through an authenticated historical release hash or an existing valid install receipt.
6. Inspect shell aliases/functions separately (`type -a` in an attended diagnostic, without invoking the definition). An alias/function named `wg` makes the compatibility alias ineffective in that shell and must be reported, not edited automatically.
7. Before and after install, print the complete ordered resolution table for ordinary shell, installer destination, and (where observable) sudo/systemd contexts. A private alias is “available” only if it resolves to the expected receipt-owned bytes.

Identity classification drives the only safe alias operation:

| Existing destination/PATH state | Alias action |
|---|---|
| No `wg` in destination or PATH | Offer explicit alias opt-in; default depends on approved outcome/package channel. |
| `wg` is same inode/symlink/hash and same version | No-op. |
| Authenticated older WorksGood `wg` in the **same receipt-owned destination** | Back up and upgrade atomically after showing old/new; never use generic Cargo `--force`. |
| WireGuard-owned or any foreign package-owned `wg` anywhere relevant | Do not create a normal-PATH alias. Install canonical `worksg` only. |
| Unknown/unverifiable `wg` | Preserve and refuse alias; provide diagnostics. |
| Multiple `wg` candidates | Preserve all; alias is unsafe unless every earlier candidate is the same verified WorksGood build and no WireGuard candidate is being masked. |

## Dual-bin feasibility prototype

A credential-free disposable Cargo prototype (`/tmp/cargo-bin-collision.eaCrJW`) used a tiny package with `worksg` and `wg` binaries:

- when a foreign `wg` already existed in the **Cargo install root**, `cargo install --path … --root …` exited 101 before installing either binary: `binary 'wg' already exists in destination; Add --force to overwrite`;
- when a foreign `wg` existed only in another PATH directory, Cargo installed both with exit 0 and did not warn; PATH order continued selecting the foreign command;
- invoking the two installed bins made `current_exe()` report the invoked installed path;
- invoking a symlink to `worksg` made Linux `current_exe()` resolve to the canonical `worksg` target.

This matches [Cargo's documented behavior](https://doc.rust-lang.org/cargo/commands/cargo-install.html): all selected executables go into one install root, and `--force` may overwrite a binary from another package. Cargo does not promise to police the rest of PATH. It proves a dual target is mechanically easy but not safely installable by default. The robust implementation choices are:

1. **Canonical binary plus symlink alias (Unix):** one executable/hash, no version skew; `current_exe()` self-spawns canonical `worksg` on the tested Linux. Cannot create the symlink when `wg` is foreign and does not map cleanly to Windows/package formats.
2. **Canonical binary plus copied alias:** portable, but two mutable files can skew across partial upgrades. Requires a versioned install directory, manifest hashes, atomic pointer switch, and repair command.
3. **Two Cargo bin targets:** `cargo install` tries all selected bins and a single destination collision aborts the package. `--bin worksg --bin nex` can avoid `wg`, but default install behavior and docs must be changed; `--force` must not be recommended. Two targets may also be updated independently.
4. **One multicall binary selected by argv[0]:** unnecessary for command behavior—the full CLI is identical—and `current_exe()` symlink resolution can erase the alias name. Use explicit command policy, not argv[0], for the bare concierge.
5. **Separate alias package/formula:** makes ownership explicit but still cannot co-install in the same profile with WireGuard. It is viable only as an opt-in package named, for example, `worksg-wg-alias`, with a hard conflict check and no dependency from core WorksGood.

## Invocation/migration inventory

A repository-wide `rg` snapshot on 2026-07-18 found standalone `wg` in **1,619 files** and 14,736 matching lines across tests/scripts/docs/prompts (an intentionally broad upper bound; many are prose or `.wg` path names). This is far beyond a README rename. The migration unit is by semantic category:

| Category | Current behavior / examples | B: canonical `worksg` requirement |
|---|---|---|
| Cargo/release metadata | `Cargo.toml` exposes `wg` + `nex`; cargo-binstall URL/archive is `wg-v…`; release workflow packages only `wg`/`nex`; shell and PowerShell installers copy those two and receipt says `wg-installer`. | Add `worksg` to build/release manifest first. Make release/install identity product-based, not inferred from basename. Install `worksg` first; conditionally create `wg`. Preserve archive/project names if desired, but manifest must enumerate alias policy/hashes. |
| Upgrade/rollback | `wg upgrade` canonicalizes `current_exe`, assumes backup file `wg`, Cargo bin dir, and messages “current wg”; it restarts daemon. | Upgrade whichever canonical owner invoked it; back up a set `{worksg,nex,owned alias}`; never restore alias over WireGuard. Rollback can restore worksg while dropping an unsafe alias. |
| Rust self-spawns | Service, pilot, spawn-task, TUI, HTML, and handlers mostly use `current_exe()`; this is the strongest migration seam. Some fallbacks still use literal `wg`. | Keep `current_exe` for children, but centralize fallback as a verified executable resolver. Never fallback to unverified PATH `wg`. Include executable identity/version in daemon IPC handshakes. |
| Generated/stored task exec | Eval/assign/publish/evolve scaffolds persist strings such as `wg evaluate run …`, `wg assign …`, `wg html publish run …`, `wg done …`. Existing graphs may contain them indefinitely. | New data should store a typed internal command or use a resolved `$WORKSG_BIN`, not a brand string. Migrate only provenance-tagged generated exec values; never rewrite arbitrary user shell. Old data requires compatibility execution independent of a PATH alias. |
| Worker wrappers/prompts | `src/service/executor.rs`, spawn context, AGENT guide, snapshots, and handlers teach agents hundreds of literal `wg …` commands. LLM shell calls therefore depend on PATH. | Inject `WORKSG_BIN`/tool calls and render one canonical name. Keep `wg_*` Pi tool names as API compatibility, but stop depending on OS `wg`. Regenerate snapshots/docs. |
| WorksGood Pi Node backend | `worksgood-pi/src/wg-backend.ts:118` calls `pi.exec("wg", …)` for every tool; human-console compat check also shells `wg pi-plugin compat-version`. A collision can send `show` to WireGuard and put network configuration/error text into the LLM. | Highest-priority blocker. Backend must receive an absolute receipt-verified WorksGood path or use canonical `worksg`; hermetic WG→Pi supplies it in env. Human-console setup records/resolves it safely. Missing canonical binary must fail loud, not silently skip compat. Re-embed the integration and bump compat if the wire contract changes. |
| Shell/scripts/tmux | `scripts/wg-connect.sh` uses `command -v wg` then `exec … "wg tui"`; daemon scripts default `WG_BIN=wg`; smoke/bench scripts use `which`, `pgrep -x wg`, `pkill`, Docker mounts `/usr/local/bin/wg`. | Resolve `WORKSG_BIN` once, verify identity, quote absolute path. Never use existence of any `wg` as WorksGood proof. Replace process-name killing with PID/service registries. Rename tmux labels only if desired; names are not execution identity. |
| systemd/service assets | `service generate-systemd` embeds absolute `current_exe` in `ExecStart` but unit name is `wg-<project>.service`; current daemon self-change logic hashes the path. | Absolute ExecStart is good. Existing units keep working through an owned alias only until regenerated. New units use canonical worksg path; old unit names may remain as stable identifiers or gain a migration alias—do not confuse with WireGuard's `wg-quick@`. |
| Completions/manpages | WorksGood currently ships no OS shell completion/man-page asset; its “manual” is Markdown/Typst. WireGuard packages already own `wg(8)` and bash completion `completions/wg`. | Never introduce WorksGood `wg(1|8)` or system `completions/wg`. New assets are `worksg` only; in-app Pi `/wg` completion is a separate namespace and may remain. |
| Docs/examples/tests | Main docs, 100+ smoke scenarios, scripts, benchmarks, generated AGENTS/CLAUDE, terminal-bench mounts, and bug reports use `wg`. | Mechanically changing prose is last, not first. Executable tests parameterize `WORKSG_BIN`; historical reports can remain historical with a note. Test both canonical and owned-alias paths throughout deprecation. |
| Packages/integrations | Pi extension package remains `@worksgood/pi`; variables are `WG_*`; graph dir is `.wg`; federation IDs use `wgid:`. | These do not collide with OS command lookup and should remain stable unless separately designed. Command spelling and product/data namespace are different concerns. |

### Old/new version skew

- A running old daemon remains old in memory after files are replaced, while its next `current_exe()` child can open the new path. Existing upgrade restarts the daemon; a name migration must make restart/compat handshake mandatory.
- Separate `worksg` and copied `wg` files can be upgraded independently by Cargo, a user copy, or old installers. Every command should report canonical path, version, build ID, alias status, and daemon build ID in `doctor/status`.
- A new plugin with an old command (or old plugin with new command) is already compat-gated for hermetic spawn, but the human console currently treats “cannot run `wg`” as nothing to assert. This must become a loud, absolute-path verification.
- Old scripts may invoke `wg` after the alias is removed; old agents may have `wg` in cached prompts; old graph `exec` values may survive for years. Removing the alias cannot be calendar-only—it needs measured migration readiness and an internal-command compatibility path.
- An old `wg` installer run after a new `worksg` install could recreate the collision. New install receipts should cause `worksg doctor` to detect legacy installers, but cannot prevent a downloaded old script. Release notes must warn and provide non-destructive repair.

## Entrypoint comparison

| Candidate | Discoverability | Collision / safety semantics | Composition | Study disposition |
|---|---|---|---|---|
| **A. Keep full CLI at `wg`; add `wg onboard`** | Minimal user/repository migration; verb appears in existing help. | Installer guard avoids immediate overwrite but `wg` remains permanently ambiguous with WireGuard, can change under PATH/module/sudo, and blocks clean distro co-installation. | Existing self-spawns mostly survive; Pi literal and scripts still need identity hardening even without rename. | **Feasible but does not resolve root collision.** Go only if maintainers consciously accept permanent namespace risk and ship strong guards/diagnostics. |
| **B. Full CLI canonical at `worksg`; optional verified `wg` alias** | New users get a unique, moderately short product command; every existing subcommand is `worksg <subcommand>`. | Resolves daily collision and package co-installation when core packages omit `wg`. Optional alias is allowed only on positively safe hosts/paths. | Highest migration cost; requires plugin/generated-exec/prompt/script/release work described above. No redispatch through PATH `wg`; `worksg` is the implementation. | **Strongest long-term collision outcome, highest transition risk.** Requires staged dual-name proof and explicit go decision. |
| **C. `worksg` concierge only; full CLI stays at `wg`** | Attractive first command; bare `worksg` could feel push-button. | First run is unique, but every printed follow-up, plugin call, agent command, and service still collides. Users must learn two command identities. | Thin concierge must invoke absolute verified `wg`; otherwise it can call WireGuard. | **Weak endpoint.** Useful only as a short-lived experiment/bridge; not a collision solution. |
| Bare `wg` wizard | High apparent discoverability only after the right `wg` wins PATH. | Official WireGuard bare `wg` means `show`. Making WorksGood bare `wg` mutate/setup magnifies ambiguity and scripts cannot know which semantics apply. | Conflates inspection and setup. | **Reject under all outcomes.** Keep WorksGood bare `wg` help-only while it exists. |
| Bare `worksg` wizard under B | Very discoverable and the name itself is an explicit product action. | No WireGuard ambiguity, but no-arg commands are commonly expected to print help; non-TTY behavior must never mutate. | Could dispatch internally to the same onboarding state machine. | **Open UX choice.** Safer baseline is help + `worksg onboard`; trial bare concierge only in attended TTY with an opt-out and no mutation before plan confirmation. |
| Extend `setup` to everything | Existing route-specific surface. | Route setup does not communicate package/auth/graph/service rollback ownership. | Would overload rather than compose the existing route primitive. | Keep as a sub-step. |
| `<canonical> up` | Familiar reconcile/start verb. | Implies start, not install/auth/package review. | Good later shorthand for validate + service reuse/start, optionally TUI. | Not first-run canonical. |
| `<canonical> start` | Understandable but generic. | Conflicts conceptually with `service start`; unclear service vs UI vs setup. | Adds synonym ambiguity. | Reject. |
| Shell alias between names | Easy for one shell. | Invisible to scripts, Pi, services, Termux, support diagnostics, and package ownership; can mask WireGuard. | No upgrade/migration contract. | Never ship as the mechanism. |
| `wsg` full CLI | Shortest candidate. | npm already has `wsg@0.0.1`; acronym is used for Web Sustainability Guidelines, Cisco WSG, grippers, and other products. Easy to mistype/forget. | Same migration cost as `worksg`, less product meaning. | Inferior to `worksg`; no-go without separate clearance. |
| `worksgood` full CLI | Clear brand and current Rust package name. | Less likely to collide but long for frequent agent/human commands; requester rejects it as too long. | Same migration cost as `worksg`. | Not preferred. |
| `wg pilot up` | Existing turnkey federation command. | Different threat model; creates identities/nodes/peers. | Must remain separate. | Never reuse for local onboarding. |

### Neutral outcome scorecard

Scores are qualitative (`best`, `middle`, `worst`) and make the trade rather than decide it:

| Criterion | A keep `wg` + guard | B canonical `worksg` + optional `wg` | C `worksg` concierge + canonical `wg` |
|---|---|---|---|
| WireGuard coexistence long-term | **worst** | **best** | **worst** |
| Near-term compatibility | **best** | **worst** until staged | middle |
| Distro/Homebrew/Nix packageability | worst | **best** | worst |
| One name for all commands | **best** but ambiguous | **best** and unique | worst |
| Existing graphs/generated exec | **best** | worst; needs compatibility engine | middle (still uses ambiguous strings) |
| Pi/plugin safety after required hardening | middle | **best** | middle |
| Future WireGuard install safety | worst | **best** | worst |
| Implementation/review size | **best** | worst | middle |

### Study recommendation (conditional, not approval)

- **Immediate release decision: no-go** on an unguarded configurator/installer under `wg`, and no-go on an immediate rename. Ship docs/read-only diagnostics first.
- **Preferred long-term go target: B, canonical full-CLI `worksg` plus a strictly optional verified `wg` compatibility alias**, *only if* name clearance and Stages 1–2 prove plugin/generated-task/package/rollback compatibility. Its configurator is `worksg onboard`; bare `worksg` remains help pending a separate attended UX test.
- **Fallback if the migration is not funded:** A with an explicit ADR accepting permanent collision risk, full identity guards, and `wg onboard`. This is containment, not a resolution.
- **Do not choose C as a steady state.** A concierge-only `worksg` makes the first command nicer but leaves the security and operability collision everywhere that matters.

This is a recommendation for what the approval decision should evaluate, not authority to rename or release.

### Alias policy common to any staged B release

- `worksg` exposes the **entire existing CLI**, not only onboarding. `worksg show`, `worksg service start`, `worksg pi-plugin`, etc. are first-class.
- `wg` is a compatibility alias to the same build only; it never has unique behavior and never forwards by searching PATH for `worksg`.
- Core deb/rpm/Homebrew/Nix/Termux packages do not ship the alias. A user installer may offer it after the non-executing identity protocol; Cargo must make it separately selectable so default installation cannot fail/overwrite because of `wg`.
- Alias creation is opt-in while WireGuard risk exists; it is never implied by `--yes`. The installer shows all PATH candidates and the exact link/copy.
- Every invocation reports a stable product/build identity independent of basename. `worksg --version` is canonical; an owned `wg --version` may add `compat alias for worksg` on stderr only when that does not break machine-readable version consumers.
- Transaction journals store a product operation and absolute executable build ID, not only the spelling typed.

## Exact proposed CLI

The verb/flags below are exact; the executable spelling is conditional on the unresolved release decision:

```text
# Outcome A or C
wg onboard [PATH] [OPTIONS]

# Outcome B
worksg onboard [PATH] [OPTIONS]
worksg <every-existing-WG-subcommand> ...

OPTIONS:
  --scope local|global
  --route pi
  --provider openrouter
  --model auto-free|<openrouter-model-id>
  --min-context <TOKENS>
  --web none|pi-web-access
  --service start|reuse|leave-stopped
  --tui
  --install-missing
  --offline
  --dry-run

<canonical> onboard --status [--transaction <ID>]
<canonical> onboard --resume [--transaction <ID>]
<canonical> onboard --rollback [--transaction <ID>]
```

Attended happy paths are therefore exactly one of:

```bash
wg onboard .       # A/C
worksg onboard .   # B
```

Under B, a separate UX experiment may make bare interactive `worksg` equivalent to `worksg onboard .`; non-TTY bare `worksg` always prints help and exits without mutation. That experiment is not required for B and must not change the transaction semantics.

The wizard shows a plan, asks the user to confirm each external-code install and the final route, starts/reuses the service only after route readiness, then separately asks (using the resolved canonical spelling):

```text
Open `worksg tui` now? [y/N]    # B
Open `wg tui` now? [y/N]        # A/C
```

Safe noninteractive example under B (replace only `worksg` with `wg` for A/C):

```bash
worksg onboard . \
  --route pi \
  --provider openrouter \
  --model nvidia/nemotron-3-super-120b-a12b:free \
  --scope local \
  --web none \
  --service reuse \
  --yes
```

Additional noninteractive rules:

- `--yes` is accepted only with `--route`, `--provider`, exact `--model` (not `auto-free`), `--scope`, `--web`, and `--service`.
- `--yes --tui` is rejected because TUI entry is attended.
- `--install-missing` must be explicit and must include an approved version policy; otherwise missing WorksGood/Pi/Node/package-manager exits with commands to run.
- `--auth existing` may be added for automation, but it only asserts that Pi already has usable auth; the probe still verifies it. There is no `--api-key` flag.
- `--dry-run` conflicts with `--resume`/`--rollback`, never installs or writes, and prints a redacted plan plus exact target paths.
- `--offline` forbids installer/package/catalog/auth network steps. It can reuse a previously tested exact route, but never manufactures a first route from stale cache.

Possible later lifecycle shorthand (shown with the outcome-neutral placeholder):

```bash
<canonical> up             # validate selected route; start or reuse service; do not open TUI
<canonical> up --tui       # same, then explicitly open TUI
```

That shortcut should be implemented only after `onboard` establishes the readiness/transaction primitives. `<canonical> tui` itself remains independent and non-onboarding. Do not add `<canonical> start` as another synonym.

## User journeys

### A. Brand-new online user

1. Install WorksGood from a verified release or package manager (see “Secure bootstrap”). Verify the receipt/hash and invoke the receipt-owned absolute path; do **not** probe an arbitrary PATH `wg`. Run `<canonical> --version` and `<canonical> dev-check` where applicable.
2. Run `<canonical> onboard .` (`worksg` under B, `wg` under A/C).
3. Read preflight: paths/versions, current directory, existing graph/config/profile/service, Node/npm/Pi availability, terminal type, network state, and planned writes.
4. If Pi is absent, choose either:
   - stop and run the printed pinned npm command; or
   - explicitly approve `--install-missing`, after seeing package/version/owner/rollback.
5. The concierge hands the terminal to Pi for `/login openrouter`. Pi owns secret input and `auth.json`; WG never reads or logs the key. Cancellation leaves the transaction `AuthPending`.
6. Refresh/list the OpenRouter catalog, apply deterministic free/tool/context filters, and show candidates with expiry and a “catalog data is advisory” warning.
7. Probe candidates through Pi. Show failures without changing execution systems. The user selects one successful exact model.
8. Show the core plugin as “embedded in this verified WorksGood build, compat X”; install/repair it with `<canonical> pi-plugin install` after confirmation. Do not call it an npm package or locate it through an unverified `wg`.
9. Offer optional `pi-web-access` separately, pinned to an exact version and integrity, with its full-system-access warning and provider/fallback behavior. Default is **none**.
10. Show final route summary. For the default project-local path, all LLM roles use the handler-first form `pi:openrouter:<vendor/model-id>` so OpenRouter auth remains Pi-owned and there is no native-WG weak-tier key or cross-handler fallback.
11. Initialize or validate the current graph. Existing graph content is never replaced.
12. Start or reuse the service. If a running daemon has a different route, ask before reload/restart.
13. Commit the transaction receipt. Ask separately whether to enter `<canonical> tui`.

### B. Existing graph with a non-Pi active profile

Preflight displays, for example:

```text
Active global profile: codex
Winning route here: codex:gpt-5.5 (profile ~/.wg/profiles/codex.toml)
Requested onboarding route: pi:openrouter:nvidia/...
```

Choices:

1. **Keep existing route and cancel Pi routing** (default). The user may still install Pi/plugin independently, but the service is not changed.
2. **Use a project-local Pi override.** Back up `.wg/config.toml`, leave the global active profile pointer intact, write explicit local role routes, and show that local wins in this project.
3. **Switch the global profile to Pi.** This calls `wg profile use pi` only after a separate global-impact confirmation and shows the local-routing cleanup backup it may create.

There is no “detected Pi, switching profile” path.

### C. Existing graph/service and partial prior setup

- Existing valid graph: skip init.
- Existing compatible console plugin: no-op.
- Existing package at the requested exact version: no-op.
- Existing service with matching graph and route: reuse it; record `service_owned=false`.
- Stale service state: use the existing service status/cleanup logic; do not delete arbitrary PID files.
- Config written but model not tested: resume at `CatalogReady`/`ProbePending`, and do not claim readiness.
- Model tested but later removed/expired: invalidate readiness, rescout, and require a new confirmation.

### D. Offline

- Fresh offline machine: verify local binaries, optionally install the embedded core plugin, and initialize graph-only. Stop at `CatalogPending`; do not select a free model or start an LLM service.
- Previously onboarded machine: validate the receipt, exact cached model identity, plugin compat, config, and daemon. Allow service reuse only if the user previously confirmed this route; label the provider probe “not revalidated offline.”
- Package install/update and Pi `/login` are unavailable. Optional web plugins remain unchanged.

### E. CI/noninteractive

CI pre-provisions Pi/auth using Pi-owned mechanisms, then runs the fully explicit command. The concierge performs a real probe but never opens `/login` or TUI. If auth is missing it exits with `AUTH_REQUIRED`, preserves the graph, and prints attended recovery instructions.

## Composition: orchestrate, do not reimplement

| Onboarding step | Owner / primitive | Concierge responsibility |
|---|---|---|
| WorksGood install/upgrade | Verified release installer or package manager | Verify canonical executable path/version/receipt and complete PATH collision table; never self-overwrite a package-manager-owned binary or foreign `wg`. |
| Route configuration | Existing setup/config/profile libraries (`<canonical> setup`) | Produce one canonical plan and route write. Do not screen-scrape a subprocess; refactor reusable planning/apply functions first. |
| Named profiles | Existing `profile show/use/pi` primitives | Detect winner/source. Preserve a non-Pi profile unless confirmed. Prefer local scope by default. |
| Graph init | Existing `init` primitive | Observe first; call only if absent. Existing graph makes init a no-op at the concierge layer, not an error. |
| Agency init | Existing `init` default / `agency init` | Ask separately; do not let agency silently introduce a native weak route. All selected roles must remain in the confirmed Pi/OpenRouter system. |
| Pi install/update | Pi's owning package manager | Use exact package/version, `--ignore-scripts` where supported, and record owner. Never copy Pi files manually. |
| Pi auth/models | Pi `/login`, `--list-models`, `pi update --models` | Hand off auth; inspect only status/model IDs. Never parse or migrate secret values. |
| WorksGood Pi integration | `<canonical> pi-plugin install/status/compat-version` | Use the embedded compat-locked plugin and pass its verified absolute CLI path to the backend. Never install a duplicate `pi-worksgood` npm package. |
| Optional web package | `pi install npm:pi-web-access@<exact>` / `pi remove` | Show source/version/integrity/full-access warning; install only on explicit opt-in. |
| Service | Existing `service status/start/reload/restart` primitives | Reuse matching daemon; record whether the transaction started it. Never stack daemons; pin the verified executable identity. |
| TUI | `<canonical> tui` | Launch only after commit and explicit attended confirmation. Never use the TUI as a setup trigger. |
| Federation pilot | Existing `pilot` primitive | Out of scope. Link to it after local onboarding; never invoke automatically. |

## Free-model discovery and test design

### Discovery

Fetch the public catalog using a bounded HTTPS client:

```text
GET https://openrouter.ai/api/v1/models?supported_parameters=tools
```

Treat the response as untrusted advisory data. Reject malformed/duplicate IDs, excessive response size, unknown schemes, and invalid numeric fields. Candidate predicate:

```text
pricing.prompt == 0
AND pricing.completion == 0
AND id ends with ":free" (or API explicitly marks the zero-price variant)
AND "text" in architecture.input_modalities
AND "tools" in supported_parameters
AND "tool_choice" in supported_parameters
AND effective_context >= configured minimum (default 128000)
AND expiration_date is absent or later than now + safety window (default 72h)
```

Use `min(catalog.context_length, top_provider.context_length)` when both exist. Prefer but do not require `structured_outputs`. Rank deterministically by:

1. non-expiring before expiring;
2. coding/agentic description or benchmark metadata;
3. effective context;
4. max completion tokens;
5. stable model ID lexical tie-break.

Do not rank on a hidden vendor preference. Show the complete scored short list and why each candidate passed.

### Probe through the selected system

Catalog metadata is not proof. Probe the exact candidate using Pi's OpenRouter provider and Pi's own credential store:

```text
pi --provider openrouter --model <id> --mode json --no-session \
   --no-extensions --no-skills --no-prompt-templates --no-context-files \
   --tools read <probe prompt>
```

Run it in a newly created, non-sensitive temp directory containing a random sentinel file. The prompt asks the model to read that file and return its random token. A successful probe requires:

- Pi resolves exactly provider `openrouter` and model `<id>`;
- at least one `read` tool event targets only the probe directory;
- the final answer includes the random sentinel;
- no write/bash/network extension tools are active;
- bounded tokens, attempts, and time;
- zero catalog price at probe time.

Because model-initiated tool calls are nondeterministic, allow two bounded attempts and report uncertainty. The smallest robust follow-up is a Pi-supported forced-tool health command; until that exists, onboarding must not overstate a text-only response as tool readiness.

Do **not** pass `--api-key`; Pi resolves `auth.json` or its environment. Do not copy Pi auth into WG secrets. Capture only redacted JSON event types and usage—not prompts that could contain secrets.

### Record

Write a non-secret route receipt only after final confirmation:

```json
{
  "schema": 1,
  "handler": "pi",
  "provider": "openrouter",
  "model": "nvidia/nemotron-3-super-120b-a12b:free",
  "wg_model_spec": "pi:openrouter:nvidia/nemotron-3-super-120b-a12b:free",
  "catalog_fetched_at": "...",
  "catalog_response_sha256": "...",
  "context_window": 1000000,
  "supported_parameters": ["tool_choice", "tools"],
  "price_prompt": "0",
  "price_completion": "0",
  "probe_passed_at": "...",
  "pi_version": "0.80.10",
  "wg_version": "...",
  "wg_pi_plugin_compat": "..."
}
```

This receipt is evidence, not dispatch authority. The explicit config/profile remains authority. On each resume/start, re-check that the winning route equals the receipt. Re-probe after model expiry, catalog removal, Pi/plugin major incompatibility, or a configurable age.

### No-key and unavailable-model behavior

- The public catalog may still be shown without a key.
- Without Pi OpenRouter auth, probe fails `AUTH_REQUIRED`; no route is written and no LLM service is started.
- Authentication cancellation leaves `AuthPending` and prints `wg onboard --resume`.
- A 404/429/no-endpoint failure does not invoke `openrouter/free` or another handler. Refresh, show another candidate, and require a new selection.
- A previously configured unavailable model pauses/rejects new dispatch according to existing provider-health policy; it is never silently replaced.

## Plugin policy

### Required core integration

The required integration is the version-locked build inside `wg`:

```bash
wg pi-plugin install
wg pi-plugin status
wg pi-plugin compat-version
```

The UI may label it “WorksGood for Pi,” but logs and receipts use `pi-worksgood`. `pi list` will not show it because it is a settings extension, not a Pi package. Onboarding must explain that distinction.

### Optional web access

As of the evidence date, npm reports `pi-web-access` 0.13.0 with integrity `sha512-ny0bHisMWdobmu1hcMp/jqjaRh6pYrH7dctBK2CVyRF4ia7bP47RnOPYdG1yiks9ohtcanWir5Hl9EFap8h0zQ==` and wildcard Pi peer dependencies. Its documentation says packages run with full access and advertises a multi-provider fallback chain. Therefore:

- default `--web none`;
- show repository, exact version, integrity, permissions, external services, browser-cookie option, and rollback before install;
- install pinned: `pi install npm:pi-web-access@0.13.0` (or the then-current reviewed version);
- do not enable browser-cookie extraction or add API keys during WG onboarding;
- do not use the package to discover the execution model;
- if its fallback chain conflicts with the user's provider policy, leave it uninstalled or configure it separately after onboarding;
- rollback only if this transaction added it: `pi remove npm:pi-web-access`.

“Compatible” means more than wildcard peer dependencies: install to a temporary Pi config root first, load it under the detected Pi version with network/browser-cookie use disabled, and reject extension load errors before changing the real settings.

## Transaction and state machine

### Journal location and lock

Use `${XDG_STATE_HOME:-~/.local/state}/wg/onboard/<transaction-id>/` so a transaction can exist before `.wg`. Contents:

```text
plan.json              # redacted immutable plan and target paths
state.json             # phase/status/ownership booleans
operations.jsonl       # append-only redacted operation results
backups/                # exact preimages of files this transaction changes
model-catalog.json      # filtered metadata only; no auth headers
route-receipt.json      # successful probe evidence
lock                    # exclusive transaction lock
```

Permissions: directory 0700, files 0600. The journal never copies `auth.json`, a keyring value, environment values, package tokens, or full process environments.

Acquire both the onboarding lock and existing WG config/service locks in a stable order. Refuse a second mutating onboarding transaction for the same canonical project path; `--status` remains readable.

### Phases

```text
Observed
  -> Planned
  -> Confirmed
  -> WgVerified
  -> PiVerified | PiInstallPending
  -> AuthReady | AuthPending
  -> CatalogReady | CatalogPending
  -> ModelProbed | ProbePending
  -> CorePluginReady
  -> OptionalPackagesReady
  -> RoutingStaged
  -> GraphReady
  -> ServiceReady | ServiceLeftStopped
  -> Committed
  -> TuiLaunched (post-commit, not rollback-critical)
```

Failure/cancellation produces `Paused { phase, code, repair }`, not “failed setup.” `--resume` re-observes every completed phase:

- executable path and version still match;
- file hashes still match expected postimages;
- auth is usable without reading secret values;
- model still exists/is free/tool-capable;
- plugin compat matches;
- package is present at the approved version;
- winning route and source match;
- graph identity/path match;
- service PID belongs to this graph and its socket responds.

If observed truth differs, invalidate that phase and all dependents, show the drift, and ask before repair.

### Commit ordering

Minimize irreversible work:

1. Observe/plan/dry-run.
2. Confirm external installs and global impact.
3. Verify/install Pi.
4. Hand off Pi auth. This is Pi-owned and intentionally not auto-rolled-back.
5. Discover/probe model without touching WG routing.
6. Install/verify core and optional packages.
7. Back up and atomically stage routing.
8. Initialize/validate graph.
9. Start/reuse service.
10. Commit receipt.
11. Optionally enter TUI.

The service starts only after model probe and config lint succeed.

## File mutations and ownership

| Path | Owner | Possible onboarding mutation | Backup / rollback |
|---|---|---|---|
| WorksGood binary dir (currently `wg`, `nex`; B adds canonical `worksg`) | Verified installer/package manager | No self-bootstrap from onboarding. Under B, install canonical first and create only a positively safe owned alias. | Owning installer/package manager; never remove/replace foreign WireGuard. |
| `~/.wg/install-receipt.toml` | WG installer | Read/verify only. | Installer owns. |
| `~/.pi/agent/auth.json` | Pi `/login` | Pi may create/update it during auth handoff. WG never reads content. | No automatic rollback; offer `/logout openrouter`/Pi UI if the user explicitly wants removal. |
| `~/.pi/agent/settings.json` | Pi/user | `wg pi-plugin install` upserts only the WG-managed extension; optional `pi install` updates packages. | Exact backup; restore only transaction-owned entry/delta, preserving concurrent user entries. |
| `~/.pi/agent/npm/...` | Pi package manager | Optional pinned package. | `pi remove` only when transaction installed it. |
| `${XDG_CACHE_HOME:-~/.cache}/wg/worksgood-pi/<compat>/` | WG | Materialized embedded core plugin. | Disposable; remove only transaction-created cache version, and only if no settings reference remains. |
| `~/.wg/profiles/*.toml`, `~/.wg/active-profile` | WG profile system | Only for explicitly confirmed global profile path. | Timestamped preimage; restore pointer/file atomically. |
| `~/.wg/config.toml` | WG | Only when user chooses global scope. | Timestamped exact preimage. |
| `.wg/config.toml` | WG project | Default route target; explicit Pi/OpenRouter roles and receipt reference. | Exact preimage; delete only if created and graph/config remains otherwise unmodified. |
| `.wg/graph.jsonl`, `.wg/executors/*`, `.wg/.gitignore` | WG init | Created only when graph absent. | Never delete if tasks/events appeared; otherwise remove transaction-created graph files after confirmation. |
| project `.gitignore`, `AGENTS.md`, `CLAUDE.md` | Project/user with WG marker | Current init may add/append WG content. | Store preimages; restore marker block or file only if unchanged since onboarding. |
| `.wg/service/*` | WG service | Socket/state/log if transaction starts service. | Stop only if `service_owned=true`; preserve logs unless user requests cleanup. Never stop a reused daemon. |
| onboarding journal | WG onboard | Always after confirmation; absent for strict dry-run. | Retain redacted audit by default; `--rollback --purge-journal` may remove after success. |

All config writes use temp-file + fsync + atomic rename. Backups are made before the first write and their hashes are journaled.

## Rollback semantics

Rollback runs compensations in reverse order:

1. Exit/ignore TUI; it is post-commit and owns no route/service setup.
2. Stop service only if this transaction started it and no later operator adopted/reconfigured it.
3. Restore route/profile/config preimages if current hashes still equal transaction postimages; otherwise stop and request manual merge.
4. Remove a newly created empty graph only if graph/config/docs have not acquired user/task changes. Otherwise preserve it and report partial rollback.
5. Remove optional package only if it was absent before and still matches the installed version.
6. Remove WG console-plugin settings entry only if absent before and unchanged; preserve the shared embedded cache if another reference uses it.
7. Never automatically delete Pi credentials. Authentication is a user/Pi-owned boundary. Print the exact attended `/logout` action if requested.
8. Never uninstall pre-existing Pi/Node/WG. If onboarding installed Pi with explicit approval, offer the owning package-manager uninstall as a **separate confirmation**, since other Pi sessions may now depend on it.

A rollback may therefore be `Complete`, `CompleteExceptCredential`, or `NeedsManualMerge`; it must never claim atomicity across an external OAuth/API-key login.

## Required edge-case behavior

| Condition | Required result |
|---|---|
| WorksGood missing / PATH resolves WireGuard | Onboarding cannot run. Use verified bootstrap instructions and the receipt-owned canonical path; do not execute/download an unverified concierge or infer WorksGood from `command -v wg`. |
| Pi/Node/npm missing | Show owner/version/path plan. Stop unless `--install-missing` was explicitly approved. Termux uses `pkg install nodejs`; desktop may use the user's package manager. |
| Offline fresh install | Graph-only initialization is allowed; catalog/auth/package steps pause. No route/service selection. |
| No OpenRouter key | Public candidates may be shown; probe cannot pass. Pause at `AuthPending`, no config route written. |
| Auth cancelled | No error budget consumed. Journal `Paused/AuthCancelled`; resume later. |
| Free model gone/expired/rate-limited | Refresh candidates and reconfirm another exact model; no random router/cross-system fallback. |
| Model claims tools but probe will not call them | Mark candidate unverified; try bounded second probe or another candidate. Do not lower the requirement silently. |
| Core plugin incompatible | Run compat status/repair. If mismatch remains, stop before routing/service. Never install an npm lookalike. |
| Optional plugin incompatible | Leave it uninstalled; core onboarding can continue only after the user confirms “continue without optional web.” |
| Existing non-Pi profile | Preserve by default. Offer local override or explicit global switch with impact/backups. |
| Existing Pi route | Compare exact route, source, auth, plugin, and probe receipt; no-op if ready. Do not replace a paid/user-chosen model just because a free candidate exists. |
| Existing graph | Validate and reuse. Never call destructive init or clear tasks. |
| Existing service, matching route | Reuse; no new PID/socket. |
| Existing service, different route | Pause and ask whether to leave it, reload after local override, or restart. Never `--force` automatically. |
| Stale service state/orphan | Use existing service diagnostics. Show PID/cmdline/graph evidence before cleanup. |
| Partial setup | Re-observe and continue from first invalid phase. Do not reinstall/re-auth/re-init blindly. |
| Non-TTY | Require fully explicit flags and pre-existing auth; never open `/login` or TUI. |
| Project-local Pi package settings | Respect Pi project trust. Default optional packages to global user scope; a shared `-l` install requires a separate repository-change confirmation. |

## Terminal and platform behavior

### tmux

- Detect existing `$TMUX`; do not nest a new tmux session.
- Warn when tmux is older than 3.5; recommend Pi's documented `extended-keys` settings.
- Onboarding prompts use ordinary Enter/Escape and must not require modified keys.
- `--tui` launches in the current pane. Session persistence is the user's tmux policy, not a hidden WG-created session.

### mosh

WG already treats mosh as an unreliable enhanced-key transport: it does not negotiate Kitty enhancement and normalizes untrusted Shift+Enter. See [`docs/bugs/tui-mosh-enter.md`](bugs/tui-mosh-enter.md).

- Explain that plain Enter submits; Ctrl+J remains the reliable multiline chord.
- The service survives TUI/PTY loss because the daemon is detached. After a mosh reconnect, run `wg tui` again; do not start another service.
- Never put an auth secret in a command that mosh/tmux history can retain.

### Termux

Pi officially supports Termux with `pkg install nodejs termux-api git`. WG's documented release targets are GNU Linux/macOS/Windows, not Android/Termux. Therefore onboarding must detect Termux (`$PREFIX`/Android) and **must not claim the current WG release installer is supported**. Until a tested Android target or source-install recipe exists, stop with a platform-specific support message. If WG is already working in Termux, continue with Pi/npm and disable assumptions about systemd, desktop browser opening, image clipboard, and native optional dependencies.

### SSH/headless

No browser is assumed. Pi `/login` may print a URL/code; onboarding shows it without opening a browser. `--tui` requires a real TTY. The service can start headlessly after a successful pre-existing-auth probe.

## Threat model

| Threat | Consequence | Mitigation |
|---|---|---|
| `curl | sh` bootstrap replacement/TOCTOU | Arbitrary code before WG exists. | Prefer package manager or inspect-first download. For WG verify SHA256 and GitHub attestations against immutable release/version. Never let a web page's mutable script be the sole trust root. |
| Installer/package typosquatting (`pi-worksgood`) | Malicious full-access extension. | Do not install nonexistent/unapproved package names. Core plugin comes from the verified WG binary. Show exact optional package source/integrity. |
| Pi package arbitrary code | Host/credential compromise. | Default optional packages off; source/version review; temp-root compatibility load; explicit consent. Pi docs explicitly say packages have full access. |
| Catalog tampering/staleness | Wrong, paid, expired, or tool-less route. | HTTPS, response limits/hash/time, strict zero-price/tool/context/expiry filter, exact Pi probe, final confirmation, short receipt TTL. |
| “Free” price changes after setup | Unexpected billing. | Recheck price before each onboarding probe and periodically before dispatch; pause on nonzero price unless the user explicitly reauthorizes paid use. |
| Random free router/provider fallback | Non-reproducible behavior/data policy change. | Pin exact tested model. No `openrouter/free` default and no cross-system fallback. |
| Secret in argv/log/history | Credential disclosure via `ps`, logs, shell history. | Pi-owned `/login`; no `--api-key`; redact headers/env; 0600 journal; never echo secret input. |
| Config/profile clobber | Loss of existing routing/customizations. | Source-aware plan, local default scope, exact backups, atomic writes, hash-guarded rollback, explicit global switch. |
| Duplicate daemons/extensions | Competing dispatch/duplicate tools. | Reuse existing service liveness checks and plugin idempotent upsert. Deduplicate package identity and extension path. |
| WireGuard/WorksGood `wg` collision | Network administration invokes graph CLI, agents/plugin invoke WireGuard, network state enters LLM output, or installers overwrite a privileged tool. | Prefer canonical `worksg` if B is approved; otherwise full PATH/owner/hash guard. Never execute foreign candidate, never force/divert, use absolute verified paths for plugin/service/generated jobs. |
| PID reuse/stale state | Kill unrelated process. | Verify process identity, graph path, socket handshake, and start time before stop/force. |
| Symlink/path confusion | Write to unintended project/config. | Canonicalize target and show physical path; refuse unsafe ownership/world-writable parent unless confirmed. Journal canonical identity. |
| Malicious project `.pi` resources | Code execution during onboarding/probe. | Probe with `--no-approve`, no project extensions/context, isolated temp cwd. Use global core plugin only after verification. |
| Rollback deletes later user work | Data loss. | Ownership flags plus pre/post hashes; refuse compensation on drift; never delete nonempty graph or reused service/package. |
| TUI-open mutation | Surprise service/provider selection. | TUI is a post-commit explicit prompt; bare canonical help/TUI remain non-onboarding. |

## Secure bootstrap recommendation

### WorksGood

Best-to-worst supported presentation:

1. OS/package manager with signed metadata, when available. Such a package must be co-installable with `wireguard-tools`; under B it exposes `worksg`, never `/usr/bin/wg`.
2. Download immutable WorksGood release archive + `SHA256SUMS` + attestation bundle; verify before extraction. Inspect its manifest for canonical binary and alias policy.
3. Download and inspect `scripts/install-wg.sh`, then run it pinned with `--version`; it verifies the archive and records ownership.
4. `curl ... | sh` may remain a convenience link, but must never be described as equivalently auditable.

The current script has checksum/attestation/receipt primitives but still unconditionally installs `wg` into its chosen directory and its archive contains only `wg`/`nex`. It is **not WireGuard-collision-safe yet**. Before any A/B/C production rollout it must perform the non-executing destination/PATH protocol; under B it installs `worksg` first and omits the alias by default. Public copy should lead with the auditable route and a post-install absolute-path/receipt check.

### Pi

Use the owning package manager and pin a tested version during an automated transaction:

```bash
npm install -g --ignore-scripts @earendil-works/pi-coding-agent@0.80.10
```

The actual approved version is resolved at plan time and printed before execution. npm verifies registry integrity; the Pi package publishes a shrinkwrap. Do not put npm tokens in command arguments. If the official Pi installer is offered, download/inspect it first; note that it may install Node and alter PATH, which requires a separate confirmation.

The concierge records Pi install owner/path/version. Upgrades remain `pi update --self` or the package manager that owns the install, not WG copying files.

## Test matrix

All implementation tests use disposable `HOME`, `XDG_STATE_HOME`, `XDG_CACHE_HOME`, `PI_CODING_AGENT_DIR`, and project roots, with inherited provider/WG variables removed.

| ID | Scenario | Assertions |
|---|---|---|
| E1 | Bare verified canonical command on fresh home | Help only; no file/network/service/TUI mutation (unless bare `worksg` concierge is separately approved, in which case no write occurs before plan confirmation). |
| E2 | `<canonical> tui` on graph-only project | Opens graph; no setup/profile/plugin/service/auth mutation; exits cleanly. UI-state writes, if any, are documented and bounded. |
| P1 | `--dry-run` fresh online | Redacted plan; no usage log, journal, cache, config, auth, package, graph, service, or TUI write. |
| P2 | Dry-run with existing config/profile/service | Exact winning sources and proposed deltas; no reload/stop. |
| I1 | WorksGood absent or `wg` resolves WireGuard | Verified install alternatives only; foreign command is never executed; no unaudited script execution. |
| I2 | Pi absent, install declined | Paused with exact pinned owner command; no later phase. |
| I3 | Pi installed by npm | Correct path/version/owner receipt; repeat is no-op. |
| A1 | OpenRouter `/login` success | WG never receives secret; auth file mode 0600; probe can use it. |
| A2 | Login cancellation | `AuthPending`; no route/service; resume works. |
| A3 | No key/non-TTY | Stable `AUTH_REQUIRED`; no prompt or mutation. |
| M1 | Public catalog normal | Only zero-price text/tool/tool-choice/context/expiry-qualified models displayed. |
| M2 | Malformed/oversize/duplicate catalog | Fail closed; no route. |
| M3 | Candidate expires inside safety window | Excluded with reason. |
| M4 | Catalog says tools, text probe only | Not accepted as tool-ready. |
| M5 | Tool probe succeeds | Exact provider/model/tool/sentinel and bounded permissions recorded. |
| M6 | 404/429/model removed | No cross-system/random fallback; rescout requires reconfirmation. |
| M7 | Price becomes nonzero | Pause before dispatch/config update; explicit paid reauthorization required. |
| G1 | Core plugin fresh install | Embedded compat path wired once; `pi list` distinction explained. |
| G2 | Core plugin repeat/corruption/mismatch | No duplicate; repair succeeds; incompatible remains loud and blocks service. |
| W1 | Optional web declined | No Pi package/settings changes. |
| W2 | Optional web accepted | Exact pinned integrity/source shown; temp compatibility load passes; one package entry only. |
| W3 | Optional package incompatible | Core flow continues only after “without web” confirmation; no partial package entry. |
| C1 | Existing non-Pi profile, default choice | No switch/local override/service reload. |
| C2 | Explicit local Pi override | Global profile preserved; local backup/write; source resolver shows local winner. |
| C3 | Explicit global Pi switch | Global impact shown; profile/local-cleanup backups; plugin ensure and reload once. |
| C4 | Current Pi setup template lint | Onboarding output is lint-clean; no deprecated executor/bare-provider route. |
| GPH1 | No graph | Init once; exact files recorded. |
| GPH2 | Existing graph with tasks | Reuse; graph bytes/tasks preserved. |
| S1 | Service stopped | Starts after readiness only; one PID/socket. |
| S2 | Matching service running | Reused; second run creates no PID/socket/handler duplicate. |
| S3 | Different-route service running | No reload/restart without confirmation. |
| S4 | Stale state/orphan/PID reuse | Identity-safe diagnosis/cleanup; unrelated process not killed. |
| R1 | Crash after every phase | Resume re-observes and completes without duplication. |
| R2 | Rollback fresh transaction | Removes/restores only owned artifacts; credential explicitly remains. |
| R3 | File modified after onboarding write | Rollback refuses overwrite and reports manual merge. |
| R4 | Graph gains a task after init | Rollback preserves graph and reports partial rollback. |
| O1 | Fresh offline | Graph-only allowed; no route/service; `CatalogPending`. |
| O2 | Offline with valid prior receipt | Existing explicit route retained; no refresh/install; readiness labeled not revalidated. |
| T1 | tmux 3.5+ | Real PTY flow, modified keys unaffected, no nested tmux. |
| T2 | mosh marker + tmux | Plain Enter works; daemon survives TUI loss; reconnect reuses service. |
| T3 | headless | No auth/TUI dialogs; explicit noninteractive result. |
| T4 | Termux detection | No false promise of supported WG release; Pi-specific instructions are correct. |
| SEC1 | Secret canary | Canary absent from argv (`/proc`), logs, journal, backups, errors, and shell history. |
| SEC2 | Malicious project `.pi` extension | Probe never loads it (`--no-approve`/isolated cwd). |
| NAME0 | A/B/C decision not approved | No executable/name behavior changes are made. |
| NAME1 | Dual full-CLI names during approved B migration | Same build ID/command tree; `worksg` is canonical, `wg` is explicitly an owned compatibility alias only; journal uses product build identity. |

Additional name/collision matrix cases are release-blocking:

| ID | Scenario | Assertions |
|---|---|---|
| N1 | `/usr/bin/wg` owned by `wireguard-tools`, user bin earlier | Install/upgrade creates `worksg` only; never runs/replaces/masks WireGuard; full CLI works via worksg. |
| N2 | Unknown executable/symlink/function named `wg` | Classified without execution; alias refused; bytes untouched. |
| N3 | Authenticated old WorksGood `wg` in receipt root | Atomic dual-name upgrade; hashes/build IDs agree; daemon restarted; rollback restores only owned files. |
| N4 | Cargo root contains foreign `wg` | Default core install can select `worksg,nex` and succeeds without `--force`; foreign file unchanged. |
| N5 | Foreign `wg` elsewhere on PATH | Complete ordered PATH table catches what Cargo misses; no alias offered as safe. |
| N6 | WireGuard installed after WorksGood | `worksg` remains stable; doctor reports resolution change/shadow; no automatic relink. |
| N7 | Homebrew/Nix profile co-install | Core WorksGood package and `wireguard-tools` co-install because core exposes no `wg`; completions/manpages do not collide. |
| N8 | Pi human console with WireGuard first on PATH | Plugin uses absolute verified worksg path and never executes WireGuard `show`; compat mismatch/missing path fails loud. |
| N9 | Existing provenance-tagged task exec `wg evaluate …` | Compatibility engine runs the owning build without global alias; arbitrary user shell containing `wg` is not rewritten. |
| N10 | Old daemon + new canonical binary | Version/build handshake rejects mixed parent/child or forces one controlled restart. |
| N11 | Copied alias skew | Doctor detects hash/version mismatch and refuses mutating commands through stale alias until repair. |
| N12 | sudo/cron/systemd/tmux/module PATH differs | Absolute service/plugin/generated paths remain WorksGood; interactive diagnostic reports different `wg` winners. |
| N13 | WorksGood uninstall with WireGuard present | Only receipt-matched worksg/alias removed; `/usr/bin/wg`, package DB, config, and completion remain byte-identical. |
| N14 | Alias rollback after user replaces it | Hash guard refuses deletion/restoration and reports manual merge. |
| N15 | `worksg --help` during dual-name release | Full current command tree is present and command name/help/examples are worksg, not hard-coded wg. |
| N16 | Windows `wg.exe` from WireGuard | Installer identifies signed foreign path without execution; installs `worksg.exe`; no PATH/file replacement. |

A permanent PTY smoke scenario must exercise the attended happy path and cancellation/resume. Unit/CLI-only tests are insufficient for `/login` handoff, confirmation defaults, mosh/tmux keys, or final TUI entry.

## Staged executable-name release plan (only if outcome B is approved)

There is no safe flag-day rename. Stage transitions are evidence-gated, and the compatibility alias is never installed over a collision.

### Stage 0 — decide and reserve (no release change)

- Maintainers choose A/B/C in an ADR and record risk acceptance.
- Perform trademark/package/domain review for `worksg`; npm absence and an inconclusive crates.io API probe are not clearance. Reserve relevant registries/accounts if appropriate.
- Freeze the compatibility promise: `.wg`, `WG_*`, `wgid:`, Pi `/wg`, and `wg_*` tool names remain stable; only OS executable lookup is in scope.
- Build a machine-readable invocation inventory with owners and tests. The broad `rg` count in this study is a baseline, not completion proof.

### Stage 1 — identity hardening under the current name

This is valuable under every outcome:

- add a read-only doctor that enumerates all command candidates/owners/hashes without executing foreign `wg`;
- make installers refuse unknown/foreign destination files and stop recommending bare Cargo `--force`;
- replace Pi backend literal PATH lookup with an injected absolute verified WorksGood path;
- centralize production Rust executable resolution; replace literal fallback and process-name kill logic;
- add build ID/absolute path to daemon/plugin/status handshakes;
- parameterize executable tests/scripts with `WORKSG_BIN`.

Do not yet change docs' canonical command or emit rename warnings.

### Stage 2 — dual-name technical preview

- Add `worksg` as a **full CLI**. `Cli` currently has `#[command(name = "wg")]`; help/error rendering must become build/invocation-policy aware rather than merely compiling the same source under a second bin name.
- Release archives/manifests contain `worksg`, `nex`, and—only for legacy channels—an authenticated alias payload. Core package-manager outputs expose `worksg`/`nex`, not `wg`.
- Existing receipt-owned `wg` installations may receive a same-build alias after an explicit plan. New/colliding hosts install no alias.
- Both names must pass identical command/JSON/config/service compatibility tests. Documentation still leads with `wg` for this preview but advertises `worksg` for testing.
- Generated internal commands stop storing literal command names before any alias deprecation begins.

### Stage 3 — canonical flip

Gate on: plugin backend migrated/re-embedded, generated exec compatibility shipped, installers safe on all supported managers, full smoke matrix green, and at least one release of dual-name field use.

- Documentation, diagnostics, prompts, generated commands, and `--help` lead with `worksg`.
- `worksg onboard` is the exact configurator entrypoint. Bare `worksg` remains help-only unless the separately approved attended experiment succeeds.
- An authenticated WorksGood `wg` alias continues without semantic difference. When stderr is a TTY it may print a rate-limited migration note; machine-readable/JSON/piped invocations stay clean.
- New installs default to no `wg` alias; the user installer offers explicit opt-in only on a collision-free resolution table. Distro/Homebrew/Nix/Termux packages never contain it.

### Stage 4 — alias retirement, not deadline deletion

- Stop offering the alias once migration readiness—not elapsed time—shows that plugin, generated tasks, integrations, docs, and supported upgrade floors no longer require it.
- Preserve an explicit opt-in alias package/link for legacy private environments where WireGuard is absent.
- Never delete a pre-existing unknown/foreign `wg`. An owned alias may be removed only by receipt/hash match.
- Historical docs/reports need not be rewritten, but active guides and copy-paste snippets must be clean.

### Migration rollback

- Before canonical flip, rollback to the last dual-name release restores `worksg` plus only a previously owned safe alias; graph/config/data namespaces are unchanged.
- Removing a newly created alias is independently reversible and does not uninstall `worksg`.
- If WireGuard appears after backup, rollback **drops/refuses the alias** rather than overwriting WireGuard. This is a successful safety-preserving rollback with a reported compatibility exception.
- A normal supported rollback floor is the first dual-name/absolute-plugin-path release. Rolling back to a pre-dual build is not automatically safe: the old plugin shells literal `wg`, help hard-codes the name, and old generated commands need the alias. Offer a quarantined versioned absolute binary/private PATH recipe only after explicit confirmation; never recreate a global collision.
- On daemon rollback, stop via the responding socket/PID record, atomically restore verified binaries, restart by absolute canonical path, and verify build handshake before dispatch.
- Package-manager-owned installs use that manager's atomic generation/rollback (especially Nix/Homebrew); the WorksGood self-upgrader must refuse ownership it does not control.

## Smallest implementation slices

No slice should start until a maintainer approves this design. Executable-name work additionally requires the A/B/C ADR.

1. **Docs first (can ship before code):** update install copy to lead with inspect/verify; warn about WireGuard collision and absolute-path verification; document the current manual Pi + plugin + graph + service flow; state that `pi-worksgood` is not a published package; document free-model volatility and no-fallback policy.
2. **Read-only identity/onboarding doctor:** under the currently verified binary, report all PATH candidates without executing them, package owners/hashes/install receipt, terminal/platform, graph/config/profile/service/plugin state, and redacted readiness. No writes/model selection. Its eventual spelling follows A/B/C.
3. **Absolute Pi backend path:** eliminate `pi.exec("wg", …)` and make missing/mismatched CLI identity loud. This should precede any new alias or canonical flip.
4. **Pi auth/probe primitive:** preferably upstream `pi auth status` plus a forced-tool `pi doctor model`; otherwise a narrowly scoped WorksGood probe wrapper using Pi JSON mode and isolated cwd.
5. **Free-model scout:** public catalog parser/filter/ranker plus receipt type. Read-only by default; no `--apply` in the first slice.
6. **Canonical Pi route writer:** one reusable config transaction that writes lint-clean handler-first Pi routes for every relevant role, with local/global preview/backups. Fix current Pi setup warnings first.
7. **Plugin/package plan:** compose the embedded `pi-plugin` primitive and Pi package manager with exact ownership/idempotency/compat tests. Optional packages remain off.
8. **Onboarding journal/rollback engine:** redacted state machine, locks, atomic backups, resume/compensation tests.
9. **`<canonical> onboard` attended UI:** orchestration only; use the primitives above. Do not add `up` yet.
10. **Service/TUI finish:** matching-daemon reuse, route mismatch confirmation, post-commit `--tui`, PTY/mosh/tmux smoke.
11. **If B is approved, execute Stages 1–4 separately:** `worksg` must expose the full CLI; alias/package/deprecation work is not folded casually into onboarding.

Documentation that can ship before the configurator:

- verified WorksGood and Pi install alternatives, including WireGuard collision checks;
- current manual `<verified-absolute-WG-binary> init` → explicit Pi route → embedded plugin install → Pi `/login` → route check → service → TUI journey;
- explanation of WG embedded plugin versus `pi list` packages;
- OpenRouter free-model discovery checklist and warning that examples expire;
- profile/source precedence and “no silent switch” guidance;
- mosh/tmux/Termux limitations;
- rollback steps for the manual flow.

## Approval gate and open decisions

**Maintainer approval is required before any implementation task is created or production code changed.** Approval must explicitly cover:

1. an ADR choosing **A keep `wg` + guard**, **B canonical full-CLI `worksg` + optional verified `wg`**, or **C `worksg` concierge + canonical `wg`**, including explicit WireGuard risk acceptance;
2. legal/ecosystem clearance for `worksg` if B/C, plus whether bare interactive `worksg` is help or concierge;
3. the exact configurator spelling (`wg onboard` for A/C or `worksg onboard` for B);
4. the alias default by install channel and the supported rollback floor;
5. whether local scope is the first-run default;
6. whether agency initializes by default and, if so, whether all agency roles must remain on Pi/OpenRouter;
7. the minimum context/expiry safety thresholds;
8. whether a two-attempt model-driven tool probe is sufficient before an upstream forced-tool primitive exists;
9. whether WorksGood may invoke npm under `--install-missing` or should only print owner commands;
10. the optional web package/version/source allowlist.

Until those decisions are approved, the recommendation is documentation and further read-only diagnostics/prototypes only. **No rename, dual-bin release, alias install, deprecation, or configurator implementation is authorized by this document.**
