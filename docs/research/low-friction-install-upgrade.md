# Low-Friction WG Install and Upgrade Path

**Task:** `research-low-friction`  
**Date:** 2026-05-28  
**Status:** Research/design recommendation  

## Summary

WG should stop making the first useful experience depend on a Rust toolchain.
Keep `cargo install --path . --locked` for contributors, but make the user path:

```bash
curl -fsSL https://install.graphwork.dev/wg.sh | sh
wg setup
cd my-project
wg init
wg service start
wg tui
```

The recommended distribution stack is:

1. **Primary:** prebuilt native `wg` + `nex` binaries on GitHub Releases for
   Linux, macOS, and Windows, with checksums and GitHub artifact attestations.
2. **Primary install UX:** a WG-owned installer script for macOS/Linux and a
   PowerShell installer for Windows. The script must verify artifacts, avoid
   sudo by default, install both binaries, write an install receipt, and print
   the short next-step flow into `wg tui`.
3. **Primary upgrade UX:** `wg upgrade`, only for WG-managed installs. It should
   detect package-manager installs and delegate to their owner (`brew upgrade`,
   `cargo binstall`, distro package manager, etc.). For WG-managed installs it
   should back up the binary/config, replace atomically, run migrations, and
   support rollback.
4. **Secondary channels:** Homebrew tap, `cargo binstall`, `cargo install`,
   direct GitHub release downloads, Nix flake, and devcontainer/container
   images. These serve users who already live in those ecosystems, but they
   should not be the beginner happy path.

This is intentionally similar to the current Codex CLI pattern: OpenAI now
documents a standalone macOS/Linux installer and "rerun installer to upgrade"
flow for Codex CLI, with npm and Homebrew as alternate tabs. WG should match
that level of simplicity while adding stronger migration and old-install
handling than Codex needs.

## Current WG Baseline

Relevant current surfaces:

| Area | Current evidence | Implication |
| --- | --- | --- |
| README beginner path | `README.md:91-112` still starts with `cargo install --git`, then `wg init`, route selection, service, TUI. | Too Rust-heavy for non-contributors. Replace public docs after binaries/installer exist. |
| Binary targets | `Cargo.toml:9-17` defines `wg` and `nex`. | Release artifacts and installers must ship both binaries together. |
| Route setup | `src/commands/setup.rs:1011-1160` supports route-driven `wg setup --route ... --yes`, scope, dry-run, and backups. | Installer can end by pointing users to `wg setup`, not by writing config itself. |
| Config migration | `src/commands/migrate.rs:515-675` implements `wg migrate config`; `src/commands/config_cmd.rs:2574-2685` reuses it for `wg config lint`. | `wg upgrade` can orchestrate existing lint/migrate rather than reimplementing config rewrites. |
| Named profiles | `src/commands/profile_cmd.rs:15-53` supports model-qualified profile activation like `codex:gpt-5.5`; `docs/design-named-profiles.md:234-254` defines the UX. | Easy path should use `wg profile use codex:gpt-5.5` when the user wants an exact worker model. |
| Handler derivation | `src/dispatch/handler_for_model.rs:26-43` maps model prefixes to handlers and deprecates `local:` / `oai-compat:` in favor of `nex:`. | Setup should teach "model route" instead of "executor/provider" and migrations should rewrite old aliases. |
| Executor discovery | `src/executor_discovery.rs:1-10` probes CLI executors and treats native/nex as always available. | `wg setup`/`wg doctor` can detect missing `claude`/`codex` binaries before the service fails. |
| Endpoint test | `src/commands/endpoints.rs:328-430` tests `/models` only. | OpenRouter/local setup should prove a generation works, not only model listing. |
| Dev freshness | `src/commands/dev_check.rs:33-45` and `:84-109` already detect stale repo-installed binaries. | Developer update flow should include `wg dev-check`; `wg upgrade` should reuse this install-source thinking. |
| Standalone nex split | `docs/guides/standalone-nex.md:86-150` distinguishes standalone `.nex/` from WG-integrated `.wg/nex/`. | Installer should ship `nex`, but setup text must keep standalone `nex` and WG routing separate. |

External references used for distribution/security context:

- OpenAI Codex CLI docs: standalone installer, first-run auth, rerun installer
  to upgrade: <https://developers.openai.com/codex/cli>
- Anthropic Claude Code install docs: Homebrew/WinGet/package-manager installs
  do not auto-update and tell users to run the owning upgrade command:
  <https://code.claude.com/docs/en/installation>
- Cargo install docs: installs executable Cargo packages into a local bin root:
  <https://doc.rust-lang.org/cargo/commands/cargo-install.html>
- cargo-binstall: binary install path for Rust projects using existing release
  artifacts: <https://github.com/cargo-bins/cargo-binstall>
- cargo-dist: can emit shell, PowerShell, npm, Homebrew, and MSI installer
  artifacts: <https://axodotdev.github.io/cargo-dist/book/reference/config.html>
- Homebrew taps: third-party formula repos are cloned and then updated through
  `brew update`: <https://docs.brew.sh/Taps>
- GitHub artifact attestations and release verification:
  <https://docs.github.com/en/actions/how-tos/secure-your-work/use-artifact-attestations/use-artifact-attestations>
  and
  <https://docs.github.com/en/code-security/how-tos/secure-your-supply-chain/secure-your-dependencies/verifying-the-integrity-of-a-release>
- OpenRouter chat completions endpoint and bearer auth:
  <https://openrouter.ai/docs/api-reference/chat-completion>
- Devcontainer and Docker security context:
  <https://docs.github.com/en/codespaces/setting-up-your-project-for-codespaces/adding-a-dev-container-configuration/introduction-to-dev-containers>
  and <https://docs.docker.com/reference/cli/docker/>

## Distribution Options

| Option | User friction | Reliability | Security posture | Platform support | WG effort | Recommendation |
| --- | --- | --- | --- | --- | --- | --- |
| Prebuilt native binaries on GitHub Releases | Low after links/scripts exist. User downloads or installer fetches one archive. | High if CI builds every target on tags. No local Rust needed. | Good with checksums, immutable releases, artifact attestations, and signed tags. Raw downloads without verification are weaker. | Linux x86_64/aarch64, macOS x86_64/aarch64, Windows x86_64 first; musl can follow. | Medium. Need release CI and target matrix. | **Foundation. Do this first.** Every other low-friction route can point at these artifacts. |
| WG-owned `curl | sh` / PowerShell installer | Lowest for beginners. One command plus `wg setup`. | High if script is thin and reads a release manifest. | Mixed: pipe-to-shell is risky. Mitigate with HTTPS, no sudo by default, visible `curl -O && sh` alternative, checksum/provenance verification, explicit version/channel. | macOS/Linux via shell; Windows via PowerShell. | Medium after release artifacts. | **Primary beginner path**, but never the only documented path. |
| `wg upgrade` / self-update | Lowest upgrade friction for WG-managed installs. | High if install receipts identify source/channel and replacement is atomic. | Needs careful boundaries: only self-update WG-managed installs, verify artifacts, back up binary/config, refuse package-manager-owned paths. | Same as native artifacts. | Medium/high. More edge cases than install. | **Add after release artifacts.** This is essential for old-version rescue. |
| `cargo install` from git/crates.io | Medium/high. Requires Rust, toolchain, build deps, time. | Good for Rust users; failures can be slow and noisy. | Builds from source, uses Cargo checksums for registry crates; git installs depend on tag/rev trust. | Any Rust-supported target, but local dependencies vary. | Low. Already works for local path; publishing crate improves it. | Keep as **developer/Rust-user fallback**, not beginner path. |
| `cargo binstall` | Low for Rust users who already have Rust. | Good if WG release artifacts match expected naming; falls back to compile when needed. | Better with checksums/provenance in release artifacts; still depends on crates.io/GitHub metadata. | Broad Rust target support where artifacts exist. | Low/medium once releases exist. | Support as **secondary** by making release assets compatible. |
| Homebrew tap | Low for macOS/Linux users with Homebrew. | High; familiar upgrade/uninstall lifecycle. | Good package-manager UX, but users trust tap formula plus release artifacts. Use checksums in formula. | macOS x86_64/aarch64, Linux x86_64/aarch64 if bottles built. | Medium. Need tap repo, bottles, updates. | Add in **stage 2** after raw releases. |
| Debian/RPM packages | Low for server operators. | High when integrated with apt/dnf; service units can be bundled later. | Strong if repo signing is done; `.deb`/`.rpm` files alone are weaker than signed repos. | Linux distros; more matrix/testing burden. | High. | Stage 3, once CLI install path is stable. |
| Nix flake + cache | Low for Nix users, high for everyone else. | High and reproducible with binary cache. | Strong when flake inputs are pinned and cache signing is configured. | Nix-supported OSes. | Medium. | Secondary/developer route; not beginner path. |
| npm wrapper | Low only for users with Node/npm. | Medium. Adds Node dependency for a Rust CLI unless it only dispatches prebuilt binaries. | npm supply chain and postinstall behavior need scrutiny. | Broad if package maps platform binaries correctly. | Medium/high. | Defer. Codex may use npm, but WG should avoid requiring Node. |
| Container/devcontainer | Low for demos/Codespaces, high for local TUI/service use. | Good for reproducible environments. | Do not put API keys in image/env defaults. Docker docs warn env/proxy values can be stored in plain text in container config. | Any Docker/Codespaces environment. | Medium. | Useful for demos and contributors, not primary install. |
| Direct GitHub release download | Medium. User must pick arch and install path. | High if artifacts are present. | Good only if checksums/attestations are verified. | Same as artifacts. | Low after release CI. | Document as explicit/manual fallback. |

## Recommended Release Shape

### Artifact naming

Ship archives named predictably:

```text
wg-vX.Y.Z-x86_64-unknown-linux-gnu.tar.gz
wg-vX.Y.Z-aarch64-unknown-linux-gnu.tar.gz
wg-vX.Y.Z-x86_64-apple-darwin.tar.gz
wg-vX.Y.Z-aarch64-apple-darwin.tar.gz
wg-vX.Y.Z-x86_64-pc-windows-msvc.zip
SHA256SUMS
SHA256SUMS.sig or GitHub release attestation
release-manifest.json
```

Each archive should contain:

```text
wg
nex
README-install.txt
LICENSE
completions/       # optional later
man/               # optional later
```

### Release tooling

Use `cargo-dist` or an equivalent release workflow generator as the starting
point, because it already understands multi-target archives and can emit shell,
PowerShell, npm, Homebrew, and MSI installer surfaces. If the generated installer
is too generic, keep the artifact workflow and replace the public installer with
a WG-owned script that reads `release-manifest.json`.

Required release properties:

- Builds must run from signed, immutable tags.
- Release assets must include checksums.
- GitHub artifact attestations should be generated for each binary archive.
- The release manifest should include version, channel, target triples, asset
  URLs, hashes, and minimum compatible WG schema/migration version.
- Assets should be compatible with `cargo binstall` where practical.
- Stable and nightly artifacts must be separate channels, not overwritten
  mutable files.

### Channels

| Channel | Source | Intended user | Retention |
| --- | --- | --- | --- |
| `stable` | SemVer tags like `v0.2.0` | Normal users | Keep indefinitely. |
| `nightly` | Main branch CI after smoke/test pass | Early adopters and WG dogfooding | Keep last N builds. |
| `dev` | Local checkout via `cargo install --path . --locked` | Contributors | Not published as a user channel. |

Avoid a hidden "latest" mutable install by default. The installer can default to
the latest stable release, but it should resolve that to an immutable version and
record it in the install receipt.

## Installer Design

Proposed user commands:

```bash
# Fast path.
curl -fsSL https://install.graphwork.dev/wg.sh | sh

# Auditable path for users who dislike piping to a shell.
curl -fsSLO https://install.graphwork.dev/wg.sh
less wg.sh
sh wg.sh --channel stable

# Explicit version/channel/install dir.
sh wg.sh --version v0.2.0 --install-dir "$HOME/.local/bin"
sh wg.sh --channel nightly --install-dir "$HOME/.local/bin"
```

Installer requirements:

- Detect OS/arch and choose the correct archive.
- Install `wg` and `nex`.
- Prefer a user-writable install dir in this order:
  `~/.local/bin`, `~/bin`, then prompt for another path. Do not use sudo unless
  the user explicitly opts into a system install.
- Verify SHA256 before extraction.
- Verify GitHub attestation when `gh` is installed, and clearly print whether
  attestation verification ran or was skipped.
- Install atomically by extracting into a temp dir and renaming into place.
- Write a receipt, for example:

```toml
# ~/.wg/install-receipt.toml
manager = "wg-installer"
version = "0.2.0"
channel = "stable"
target = "x86_64-unknown-linux-gnu"
installed_at = "2026-05-28T00:00:00Z"
binary_dir = "/home/user/.local/bin"
release_url = "https://github.com/graphwork/wg/releases/tag/v0.2.0"
artifact_sha256 = "..."
```

- Print the next steps:

```text
WG installed:
  wg  /home/user/.local/bin/wg
  nex /home/user/.local/bin/nex

Next:
  wg setup
  cd your-project
  wg init
  wg service start
  wg tui
```

## Upgrade Strategy

WG should add `wg upgrade` and keep installer reruns as the fallback:

```bash
wg upgrade
wg upgrade --dry-run
wg upgrade --channel nightly
wg upgrade --version v0.2.0
wg upgrade --rollback
```

### Install-source detection

`wg upgrade` should first classify the current binary:

| Source | Detection | Action |
| --- | --- | --- |
| WG installer | `~/.wg/install-receipt.toml` points at this executable. | Self-update supported. |
| Homebrew | executable path under Homebrew prefix or receipt marker. | Refuse self-update; print `brew upgrade graphwork/tap/wg`. |
| Cargo install | path under Cargo bin dir and no WG receipt. | Refuse self-update by default; print `cargo install --git ... --locked` or `cargo binstall workgraph`. |
| Debian/RPM/Nix | package manager owns path or Nix store path. | Refuse self-update; print owner command. |
| Developer checkout | `wg dev-check` detects repo binary freshness. | Print developer update flow. |
| Unknown copied binary | no receipt and path is writable. | Offer reinstall via installer with explicit confirmation, not silent self-update. |

### Safe update sequence

For WG-managed installs:

1. Resolve the target release and channel.
2. Download manifest and archive to a temp directory.
3. Verify checksum and attestation.
4. Run preflight:
   - `wg service status`
   - `wg config lint`
   - `wg migrate config --dry-run`
   - `wg migrate secrets --dry-run`
   - graph layout scan for `.workgraph`, old `.wg`, missing schema marker,
     stale active profile, stale starter profiles, and deprecated model prefixes.
5. Ask for confirmation unless `--yes` was passed.
6. Stop or pause daemon if needed. Do not kill running workers without telling
   the user what will happen.
7. Back up current binary dir to `~/.wg/backups/bin/<timestamp>/`.
8. Replace `wg` and `nex` atomically.
9. Run migrations:
   - `wg migrate config --all`
   - `wg migrate secrets` only with explicit key-store confirmation
   - graph layout migrations with backups
   - `wg profile init-starters --force` only after showing diff
10. Restart/reload daemon if it was running.
11. Print validation:
    - `wg --version`
    - `nex --version`
    - `wg config lint`
    - `wg service status`
    - `wg tui` as the final manual check.

### Old-version migration policy

Old WG versions are the hardest case and should be treated as product surface,
not incidental migration errors.

`wg upgrade` should detect and handle:

- `.workgraph/` layouts from early versions. Default action: copy to `.wg/`
  with a timestamped backup, never destructive rename on the first pass.
- Deprecated config keys: `agent.executor`, `dispatcher.executor`,
  compactor keys, old verify keys, `chat_agent`, `max_chats`, stale Codex model
  strings, stale OpenRouter Claude model strings, and `local:` / `oai-compat:`
  prefixes where safe.
- Secret migration from `api_key_env` to `api_key_ref`, using `wg secret` and
  prompting before storing any key.
- Stale starter profiles: compare installed `~/.wg/profiles/*.toml` against
  built-in templates and offer `wg profile init-starters --force` with diff.
- Broken profile precedence: after migration, `wg profile show --json` should
  expose `agent.model`, `models.default`, `models.task_agent`, and active named
  profile so users can see what will actually run.
- Stale daemon state: stale PID, old service state format, orphaned chat locks,
  and old chat/coordinator names. Recovery should prefer "pause/reload" over
  deleting state.

## Executor and Model Setup

The easy path should talk about **routes**, not executors. A route is the model
prefix plus optional endpoint/key handling.

### Claude CLI route

Command:

```bash
wg setup --route claude-cli --yes
wg profile use claude:opus
```

Setup behavior:

- Probe `claude` on PATH before writing the route.
- If missing, show the official Claude Code install options instead of letting
  the service fail later.
- If present, probe auth with a cheap explicit command when the user agrees.
- Default worker model: `claude:opus`.
- Meta/agency roles: keep cheap pins like `claude:haiku`.

### Codex CLI route

Current Codex CLI docs offer a standalone installer and first-run sign-in.
WG should treat Codex as a CLI-auth route:

```bash
wg setup --route codex-cli --yes
wg profile use codex:gpt-5.5
```

Setup behavior:

- Probe `codex` on PATH.
- If missing, show the current official install command and do not claim setup
  is complete.
- If present, detect whether `codex` can run non-interactively and whether auth
  is configured.
- Default worker model: `codex:gpt-5.5` while that remains the current WG
  default.
- Meta/agency roles: pin to the cheaper Codex mini route already represented in
  current profile/config docs.

### OpenRouter route

Current command shape should work today with environment-backed keys:

```bash
export OPENROUTER_API_KEY="..."
wg setup --route openrouter --api-key-env OPENROUTER_API_KEY --yes
wg endpoints test openrouter
```

Recommended next shape:

```bash
wg secret set openrouter
wg setup --route openrouter --api-key-ref keyring:openrouter --yes
wg endpoints test openrouter --generate
```

Setup behavior:

- Prefer `api_key_ref = "keyring:openrouter"` over writing `api_key_env`.
- Validate the key by making an actual low-token chat completion, not only a
  `/models` request. OpenRouter documents `/chat/completions` with bearer auth;
  WG should test the path the dispatcher will actually use.
- Make model selection registry-driven so docs do not stale-pin a specific
  OpenRouter model string.
- Show cost/cap warning before the first generation probe.

### Native `nex` route

`nex` ships with WG and is therefore the only route whose executor is available
immediately after install.

```bash
wg setup --route local \
  --url http://localhost:11434/v1 \
  --model nex:qwen3-coder \
  --yes
wg endpoints test local --generate
```

Setup behavior:

- Keep standalone `.nex/` config separate from WG-integrated `.wg/nex/`, per
  `docs/guides/standalone-nex.md`.
- Make local endpoint setup explicit: URL, model name, auth if any.
- `wg nex` and TUI chat should use WG project routing; bare `nex` should use
  standalone routing.

### Profile and precedence clarity

WG should treat `wg profile use PROVIDER:MODEL` as the clearest exact-worker
command:

```bash
wg profile use claude:opus
wg profile use codex:gpt-5.5
wg profile use nex:qwen3-coder
```

Setup/profile output should always print:

```text
Next worker route:
  active profile: codex
  models.task_agent: codex:gpt-5.5
  agent.model fallback: codex:gpt-5.5
  handler: codex CLI
  endpoint: CLI-managed auth
```

This avoids asking users to reason about tiers, global config, local config,
active profiles, and role defaults before they have seen `wg tui`.

## Copy/Paste Command Flows

### Fresh machine to `wg tui`

Recommended future path:

```bash
# 1. Install WG itself.
curl -fsSL https://install.graphwork.dev/wg.sh | sh

# 2. Choose a model route.
wg setup

# 3. Start a project.
mkdir -p ~/work/my-project
cd ~/work/my-project
wg init
wg agency init

# 4. Start the daemon and open the operating surface.
wg service start
wg tui
```

Non-interactive Codex route:

```bash
curl -fsSL https://install.graphwork.dev/wg.sh | sh
wg setup --route codex-cli --yes
wg profile use codex:gpt-5.5
mkdir -p ~/work/my-project
cd ~/work/my-project
wg init
wg agency init
wg service start
wg tui
```

Non-interactive local `nex` route:

```bash
curl -fsSL https://install.graphwork.dev/wg.sh | sh
wg setup --route local \
  --url http://localhost:11434/v1 \
  --model nex:qwen3-coder \
  --yes
mkdir -p ~/work/my-project
cd ~/work/my-project
wg init
wg agency init
wg service start
wg tui
```

### Existing old WG install to current working setup

Recommended future path:

```bash
wg upgrade --dry-run
wg upgrade
wg config lint
wg profile show
wg service restart
wg tui
```

Fallback path before `wg upgrade` exists:

```bash
# Back up user and project state first.
cp -a "$HOME/.wg" "$HOME/.wg.backup.$(date +%Y%m%dT%H%M%S)" 2>/dev/null || true
cp -a .wg ".wg.backup.$(date +%Y%m%dT%H%M%S)" 2>/dev/null || true
cp -a .workgraph ".workgraph.backup.$(date +%Y%m%dT%H%M%S)" 2>/dev/null || true

# Install the new binary pair.
curl -fsSL https://install.graphwork.dev/wg.sh | sh

# Clean up old config/secrets with previews first.
wg config lint
wg migrate config --dry-run
wg migrate config --all
wg migrate secrets --dry-run
wg migrate secrets

# Refresh route/profile defaults and restart.
wg profile init-starters
wg profile show
wg service restart
wg tui
```

If a very old repo has `.workgraph/` but no `.wg/`, the future upgrade command
should handle that. Until then, users need an explicit migration guide rather
than guessing whether to rename directories.

### Developer checkout update

Contributor path remains Rust-based:

```bash
cd /path/to/wg
git switch main
git pull --ff-only
cargo install --path . --locked
wg dev-check
wg migrate config --dry-run
wg migrate config --all
wg service restart
wg tui
```

For a feature branch:

```bash
cd /path/to/wg
git switch my-branch
cargo build --locked
cargo test --locked
cargo install --path . --locked
wg dev-check
```

### First project launch into `wg tui`

```bash
mkdir -p ~/work/my-project
cd ~/work/my-project
wg init
wg agency init
wg service start
wg add "Try WG" -d "Use WG to track one small piece of work."
wg tui
```

Later, this should compress to:

```bash
mkdir -p ~/work/my-project
cd ~/work/my-project
wg launch
```

`wg launch` is a proposed convenience command that should perform safe, visible
steps: init if missing, route doctor, agency init if missing, service start if
stopped, then open `wg tui`.

## What WG Should Automate or Detect

| Check | Current state | Recommended behavior |
| --- | --- | --- |
| Missing executor binaries | `wg executors` can detect PATH presence. | `wg setup` should block or warn before selecting `claude-cli`/`codex-cli` when the binary is missing. |
| Executor auth | Not fully proven by PATH detection. | Add explicit auth/generation probes with clear cost prompt. |
| Stale installed `wg` vs repo | `wg dev-check` exists. | Keep for developers; `wg upgrade` should detect install source for normal users. |
| Old config keys | `wg config lint` and `wg migrate config` exist. | `wg upgrade` should run dry-run previews and summarize before writing. |
| Old secrets | `wg migrate secrets` exists. | Setup should prefer keyring refs so new users do not create more env-var config. |
| Stale daemon | Service commands exist. | `wg upgrade` should detect running/stale daemon and reload/restart safely. |
| API key validity | Endpoint test checks `/models`. | Add generation probe so OpenRouter/local routes prove completions work. |
| Broken profile precedence | `wg profile show` exposes active config. | Make route summary always print next worker model, fallback, handler, and endpoint. |
| Old graph layout | Not a single first-class upgrade surface. | Add graph layout scan/migrate to `wg upgrade`, with copy-first backups. |
| Install ownership | Not tracked for release binaries yet. | Installer writes receipt; `wg upgrade` refuses to self-update package-manager paths. |

## Security Requirements

Installer scripts:

- Document both pipe-to-shell and inspect-first flows.
- Use HTTPS, strict shell mode, temp dirs, no world-writable extraction targets,
  and no sudo by default.
- Pin to immutable release versions after resolving a channel.
- Verify checksums before moving binaries into PATH.
- Verify GitHub artifact attestations when `gh` is available, and make skipped
  attestation verification visible.
- Do not execute downloaded archives or generated postinstall scripts before
  verification.

Binary downloads:

- Use signed tags and GitHub immutable releases where available.
- Publish SHA256 checksums and release manifest.
- Treat GitHub Releases as the distribution source but not the trust model by
  itself; checksums and attestations are the trust aids.

Key handling:

- Prefer `wg secret set` and `api_key_ref = "keyring:<name>"`.
- Avoid inline keys in TOML.
- Avoid installer arguments that contain secrets.
- Do not put API keys into container images or devcontainer defaults. Docker
  documents that environment/proxy values can be stored in container config and
  inspected later.

Self-update:

- Never self-update package-manager-owned paths.
- Back up current binaries and config before replacement.
- Verify downloaded replacement before stopping the daemon.
- Make migrations previewable and separately confirm secret storage.
- Provide `wg upgrade --rollback` and print the backup path after every update.

## Staged Implementation Plan

### Milestone 1: Release artifacts

Goal: users can download a native archive without Rust.

Deliver:

- Tag-driven release CI for `wg` and `nex`.
- Linux/macOS/Windows target matrix.
- Checksums, release manifest, and GitHub attestations.
- Artifact naming compatible with installer and `cargo binstall`.

Follow-on WG task created: `implement-native-wg`.

### Milestone 2: Verified one-command installers

Goal: new users can install WG in one command and get next steps.

Deliver:

- `install-wg.sh` and PowerShell equivalent.
- Stable/nightly/version selection.
- User-writable install dir default.
- Verification and receipt writing.
- `docs/guides/install.md` with install and upgrade commands.

Follow-on WG task created: `implement-verified-wg`.

### Milestone 3: `wg upgrade`

Goal: old WG installs recover safely without knowing Rust, config migration, or
daemon internals.

Deliver:

- Install-source detection.
- Dry-run plan.
- Binary/config backups.
- Atomic replacement for WG-managed installs.
- Config/secrets/graph migration orchestration.
- Rollback.
- Package-manager delegation.

Follow-on WG task created: `implement-wg-upgrade`.

### Milestone 4: Route health and generation probes

Goal: setup catches real model/auth failures before the user starts the daemon.

Deliver:

- `wg endpoints test --generate` or default generation probe for OAI-compatible
  endpoints.
- Clear error taxonomy: connectivity, authentication, model-not-found,
  malformed response, generation failure.
- Setup route checks that use executor discovery plus optional auth probes.

Follow-on WG task created: `harden-endpoint-tests`.

### Milestone 5: First-run launch command

Goal: reduce the first project launch from five concepts to one safe command.

Deliver:

- Proposed `wg launch`: init project if missing, run route doctor, initialize
  agency if missing, start service if stopped, then open `wg tui`.
- TUI first-run panel can reuse the same checks later.

No task was created for this yet because it should wait until the installer and
upgrade command settle the release/install contract.

## Final Recommendation

Build the stack in this order:

1. Native release artifacts with verification material.
2. WG-owned installer scripts that install both `wg` and `nex`.
3. `wg upgrade` for WG-managed installs, with package-manager delegation.
4. Generation-grade route health checks.
5. A `wg launch` first-run convenience command.

Do not make Homebrew, npm, Debian/RPM, Nix, or containers the first path. They
are valuable secondary channels, but they are all ecosystem-specific. The lowest
friction path WG controls end-to-end is native artifacts plus a verified
installer plus a WG-aware upgrade/migration command.

## Validation Self-Check

- Native builds: addressed in Distribution Options, Recommended Release Shape,
  and Milestone 1.
- One-command install: addressed in Installer Design and command flows.
- Old-version upgrade: addressed in Upgrade Strategy and fallback old-install
  flow.
- Executor setup: addressed in Executor and Model Setup.
- Profile/model defaults: addressed in Profile and precedence clarity.
- `wg tui` first-run flow: addressed in fresh install and first project launch
  flows.
- Five-plus option comparison: Distribution Options compares ten options.
- Security implications: Security Requirements covers scripts, binaries, keys,
  containers, and self-update.
- Follow-on WG tasks: created `implement-native-wg`, `implement-verified-wg`,
  `implement-wg-upgrade`, and `harden-endpoint-tests`.
