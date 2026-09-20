# @worksgood/cli

[WorksGood](https://github.com/graphwork/wg) (`wg`, `worksgood`, `nex`) as an npm
package, plus the [@earendil-works/pi-coding-agent](https://www.npmjs.com/package/@earendil-works/pi-coding-agent)
CLI — one `npm install -g @worksgood/cli` (Node 20+) brings the whole WG+pi stack.

`cargo install --git https://github.com/graphwork/wg --locked` remains the
primary documented install path; npm is the additional channel for users who
already live in Node-land (which is exactly the WG audience — the same users
just installed pi from npm).

## How it works (zero install scripts)

This package contains **no native code and no install scripts** —
`scripts` is deliberately empty. The prebuilt per-platform binaries live in
separate packages declared as `optionalDependencies`:

- `@worksgood/linux-x64-gnu` (glibc Linux, x64)
- `@worksgood/darwin-arm64` (Apple silicon; binaries are currently **unsigned
  and un-notarized** — see the macOS note below)

npm itself picks and installs exactly the right one for your platform, and the
tiny `bin` shims (`wg`, `nex`, `worksgood`) resolve the binary inside it and
exec it with inherited stdio, forwarding the exit code. This is the pattern
esbuild, biome, and cargo-dist's npm installer all converged on — chosen
specifically so installs work with `--ignore-scripts`, offline installs, and
read-only filesystems.

## Compatibility notes

- **macOS binaries are currently unsigned and un-notarized.** Apple Developer
  ID signing/notarization secrets are not configured yet, so Gatekeeper may
  block the first run. Allow it with
  `xattr -d com.apple.quarantine "$(which wg)"` (repeat for `worksgood` and
  `nex`) or right-click → Open. This will be corrected once signing is
  configured; do not claim the macOS packages are signed or notarized.
- **`--ignore-scripts` works fully.** There are no scripts to skip, and the
  binaries arrive as ordinary package payload, not as a postinstall download.
- **`--no-optional` breaks binary resolution.** Without the optional platform
  package there is nothing to exec. The shim does not fail with a stack trace —
  it prints the documented fallback:
  `cargo install --git https://github.com/graphwork/wg --locked`.
  (The same caveat esbuild documents: the flow survives `--ignore-scripts`
  *or* `--no-optional`, but not both.)
- **musl (Alpine/void) is not covered yet.** npm's `os`/`cpu` fields cannot
  express libc, so musl support ships as separately-named `*-musl` packages in
  a later phase; musl systems currently get the cargo-install fallback.
- **`WG_BINARY_PATH` escape hatch.** Set it to an absolute path to exec that
  binary instead of the packaged one (mirrors `ESBUILD_BINARY_PATH` /
  `BIOME_BINARY`) — useful for distro-packaged or self-built binaries, e.g.
  `WG_BINARY_PATH=/usr/local/bin/wg wg status`.
- **Version pinning.** The platform packages are pinned to the *exact* release
  version (no caret) so npm can never mix a new metapackage with a skew-old
  binary — WG's binaries talk versioned graph protocols. The pi dependency is
  a caret range because pi versions (and releases) independently of WG; WG
  execs it as a sibling CLI.

## Uninstall / update

```bash
npm update -g @worksgood/cli     # update WG + platform binaries + pi
npm uninstall -g @worksgood/cli
```
