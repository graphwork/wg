# Install WG

WG ships as native release archives containing both binaries:

- `wg` - the task graph CLI, service, and TUI.
- `nex` - the standalone/native model client binary used by WG routes.

The installer picks the archive for your OS/CPU, verifies its SHA256 checksum,
verifies GitHub artifact provenance when the `gh` CLI is available, installs
both binaries into a user-writable directory, and writes
`~/.wg/install-receipt.toml`.

## Fresh Install

macOS and Linux:

```bash
curl -fsSL https://install.graphwork.dev/wg.sh | sh
```

Windows PowerShell:

```powershell
irm https://install.graphwork.dev/wg.ps1 | iex
```

The installer avoids `sudo` by default. On macOS and Linux it prefers
`$HOME/.local/bin`, then `$HOME/bin`. On Windows it prefers `$HOME\.wg\bin`.
If the selected directory is not already on `PATH`, the installer prints a
warning so you can add it before running `wg`.

After install, run the short first-project flow:

```bash
wg setup
mkdir -p ~/work/my-project
cd ~/work/my-project
wg init
wg service start
wg tui
```

If you want the default agency starter roles before opening the TUI, add:

```bash
wg agency init
```

## Auditable Install

If you do not want to pipe a remote script directly into a shell, download and
inspect it first.

macOS and Linux:

```bash
curl -fsSLO https://install.graphwork.dev/wg.sh
less wg.sh
sh wg.sh --channel stable
```

Windows PowerShell:

```powershell
iwr https://install.graphwork.dev/wg.ps1 -OutFile install-wg.ps1
notepad .\install-wg.ps1
powershell -ExecutionPolicy Bypass -File .\install-wg.ps1 -Channel stable
```

## Channels, Versions, And Custom Paths

Stable is the default channel. It resolves to the latest stable GitHub Release
and then records the immutable version/tag in the install receipt.

macOS and Linux:

```bash
# Latest stable into the default user bin directory.
curl -fsSL https://install.graphwork.dev/wg.sh | sh

# Explicit stable version.
curl -fsSL https://install.graphwork.dev/wg.sh | sh -s -- --version v0.2.0

# Nightly channel.
curl -fsSL https://install.graphwork.dev/wg.sh | sh -s -- --channel nightly

# Custom install directory.
curl -fsSL https://install.graphwork.dev/wg.sh | sh -s -- \
  --install-dir "$HOME/.local/bin"

# Show what would happen without writing binaries or receipts.
curl -fsSL https://install.graphwork.dev/wg.sh | sh -s -- --dry-run
```

Windows PowerShell:

```powershell
# Latest stable into the default user bin directory.
irm https://install.graphwork.dev/wg.ps1 | iex

# Explicit stable version.
iwr https://install.graphwork.dev/wg.ps1 -OutFile install-wg.ps1
powershell -ExecutionPolicy Bypass -File .\install-wg.ps1 -Version v0.2.0

# Nightly channel.
powershell -ExecutionPolicy Bypass -File .\install-wg.ps1 -Channel nightly

# Custom install directory.
powershell -ExecutionPolicy Bypass -File .\install-wg.ps1 `
  -InstallDir "$HOME\.local\bin"

# Show what would happen without writing binaries or receipts.
powershell -ExecutionPolicy Bypass -File .\install-wg.ps1 -DryRun
```

The `dev` channel is for contributors building from a local checkout. It
requires Rust and Cargo:

```bash
git clone https://github.com/graphwork/wg.git
cd wg
sh scripts/install-wg.sh --channel dev --dev-dir "$PWD" \
  --install-dir "$HOME/.local/bin"
```

PowerShell:

```powershell
git clone https://github.com/graphwork/wg.git
cd wg
powershell -ExecutionPolicy Bypass -File .\scripts\install-wg.ps1 `
  -Channel dev `
  -DevDir "$PWD" `
  -InstallDir "$HOME\.local\bin"
```

Contributor checkouts can also keep using:

```bash
cargo install --path . --locked
```

Use `wg dev-check` after a source install to detect branch or binary freshness
drift.

## Verification

For release channels, the installer downloads:

- `release-manifest.json`
- `SHA256SUMS`
- the native archive for your target, for example
  `wg-v0.2.0-x86_64-unknown-linux-gnu.tar.gz`

It computes the archive SHA256 locally and refuses to install if the digest does
not match `SHA256SUMS`.

If the GitHub CLI is installed and the installer is downloading from the normal
GitHub release URL, it also verifies release provenance:

```bash
gh attestation verify release-manifest.json --repo graphwork/wg
gh attestation verify SHA256SUMS --repo graphwork/wg
gh attestation verify <archive> --repo graphwork/wg
```

The output always reports the verification status:

```text
checksum: OK (...)
attestation: OK
```

or, when provenance tooling is unavailable:

```text
checksum: OK (...)
attestation: skipped (gh not installed)
```

Custom mirrors and test URLs still get SHA256 verification, but GitHub
attestation verification is skipped because the archive is no longer being read
from a GitHub Release URL.

## Reinstall Or Upgrade With The Installer

Rerunning the installer over the same install directory replaces `wg` and `nex`
atomically and rewrites the install receipt.

```bash
curl -fsSL https://install.graphwork.dev/wg.sh | sh
wg --version
nex --version
```

Windows:

```powershell
irm https://install.graphwork.dev/wg.ps1 | iex
wg --version
nex --version
```

## Existing Old WG Install

Use this flow when you already have an older WG binary or an old project layout
and want to move to the current native binary pair.

First, back up user-level and project-level WG state:

```bash
cp -a "$HOME/.wg" "$HOME/.wg.backup.$(date +%Y%m%dT%H%M%S)" 2>/dev/null || true
cp -a .wg ".wg.backup.$(date +%Y%m%dT%H%M%S)" 2>/dev/null || true
cp -a .workgraph ".workgraph.backup.$(date +%Y%m%dT%H%M%S)" 2>/dev/null || true
```

Install the new `wg` and `nex` binaries:

```bash
curl -fsSL https://install.graphwork.dev/wg.sh | sh
```

Preview and apply config migrations:

```bash
wg config lint
wg migrate config --dry-run
wg migrate config --all
wg migrate secrets --dry-run
wg migrate secrets
```

Refresh starter profiles, inspect the active route, restart the daemon, and
open the TUI:

```bash
wg profile init-starters
wg profile show
wg service restart
wg tui
```

PowerShell backup and reinstall:

```powershell
$stamp = Get-Date -Format "yyyyMMddTHHmmss"
if (Test-Path "$HOME\.wg") { Copy-Item "$HOME\.wg" "$HOME\.wg.backup.$stamp" -Recurse }
if (Test-Path ".wg") { Copy-Item ".wg" ".wg.backup.$stamp" -Recurse }
if (Test-Path ".workgraph") { Copy-Item ".workgraph" ".workgraph.backup.$stamp" -Recurse }

irm https://install.graphwork.dev/wg.ps1 | iex
wg config lint
wg migrate config --dry-run
wg migrate config --all
wg migrate secrets --dry-run
wg migrate secrets
wg profile init-starters
wg profile show
wg service restart
wg tui
```

When `wg upgrade` is available for WG-managed installs, prefer:

```bash
wg upgrade --dry-run
wg upgrade
wg config lint
wg profile show
wg service restart
wg tui
```

Until then, the installer rerun plus explicit migration commands above is the
safe old-upgrade path.

## Manual Release Download

Manual installs are useful for locked-down environments:

```bash
gh release download v0.2.0 --repo graphwork/wg
sha256sum -c SHA256SUMS
gh attestation verify wg-v0.2.0-x86_64-unknown-linux-gnu.tar.gz \
  --repo graphwork/wg
tar -xzf wg-v0.2.0-x86_64-unknown-linux-gnu.tar.gz
install -m 0755 wg-v0.2.0-x86_64-unknown-linux-gnu/wg "$HOME/.local/bin/wg"
install -m 0755 wg-v0.2.0-x86_64-unknown-linux-gnu/nex "$HOME/.local/bin/nex"
```

Use the target archive matching your host:

- `x86_64-unknown-linux-gnu`
- `aarch64-unknown-linux-gnu`
- `x86_64-apple-darwin`
- `aarch64-apple-darwin`
- `x86_64-pc-windows-msvc`

Windows ARM64 native archives are not published yet. The PowerShell installer
exits with an explicit unsupported-platform message on Windows ARM64; use the
x86_64 Windows artifact under emulation or the `dev` channel from source.
