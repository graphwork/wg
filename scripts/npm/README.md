# npm distribution for WG (`@worksgood/*`)

The **first slice** of the npm install channel, per
[`docs/research/npm-distribution.md`](../../docs/research/npm-distribution.md)
(the esbuild/biome/cargo-dist decision doc): one metapackage + one platform
package per release target, `optionalDependencies` wiring, a tiny pure-JS bin
shim, and **zero postinstall scripts**.

```
scripts/npm/
├── cli/                     @worksgood/cli metapackage SOURCE (shims only)
│   ├── bin/{wg,nex,worksgood}.js   bin shims (exec the resolved binary)
│   ├── lib/resolver.js             platform map, musl detection,
│   │                               WG_BINARY_PATH override, cargo fallback
│   └── package.json                deps: pi CLI + exact-pinned platform pkgs
├── platform/                platform-package templates (package.json.in,
│                            README.md.in) — __PLACEHOLDER__ substituted
├── test/resolver.test.js    unit tests (node --test scripts/npm/test/)
├── make-packages.sh         assemble packages from release archives (CI) or
│                            a loose bin dir (local dry run); never publishes
└── verify-install.sh        end-to-end local dry-run proof (see below)
```

First-slice platform packages (from the existing release.yml matrix):

| Rust target | npm package | status |
| --- | --- | --- |
| `x86_64-unknown-linux-gnu` | `@worksgood/linux-x64-gnu` | slice 1 |
| `aarch64-apple-darwin` | `@worksgood/darwin-arm64` | slice 1 |
| `aarch64-unknown-linux-gnu` | `@worksgood/linux-arm64-gnu` | phase 2 |
| `x86_64-apple-darwin` | `@worksgood/darwin-x64` | phase 2 |
| `x86_64-pc-windows-msvc` | `@worksgood/win32-x64` | phase 2 |
| musl variants | `@worksgood/linux-{x64,arm64}-musl` | phase 2 (runtime-detected, separately named) |

## Usage

```bash
# CI (release.yml npm-package job): from downloaded release archives
scripts/npm/make-packages.sh --archive-dir dist --out-dir npm-dist --pack

# Local dry run (bin-dir mode; uses target/release or ~/.cargo/bin by default)
scripts/npm/verify-install.sh            # full pack + clean-prefix install proof
scripts/npm/verify-install.sh --keep     # keep the work dir for inspection

# Unit tests
node --test scripts/npm/test/
```

`make-packages.sh` reads the version from **Cargo.toml** (the single source of
truth, same value `release.yml` derives) and stamps it into every package. In
archive mode, pass `--archive-version <plan-version>` when the release tag is
a `release-test-*` tag (archives are named `wg-v<plan-version>-<target>` while
the npm version stays the Cargo version — npm publish is skipped for those
runs anyway).

## Design invariants (non-negotiable)

- **Zero install scripts.** `scripts` is empty in every package and
  `make-packages.sh` hard-fails if a template ever grows an install/prepare/
  pack hook. This is the esbuild-PR-#1621 lesson: postinstall downloads break
  under `--ignore-scripts`, offline installs, and read-only filesystems.
- **`--ignore-scripts` compatibility.** A plain
  `npm install -g @worksgood/cli` and the same install with `--ignore-scripts`
  are functionally identical — there is nothing to skip; binaries are payload.
  The documented caveat (esbuild issue #4288): the flow survives
  `--ignore-scripts` **or** `--no-optional`, but not both — with
  `--no-optional` no platform binary is installed, and the shim prints the
  cargo-install fallback instead of a stack trace.
- **Exact version pins.** The metapackage pins platform packages to the exact
  release version (no caret) so npm can never mix WG protocol-skewed
  binaries; the `@earendil-works/pi-coding-agent` dependency is a caret range
  because pi versions/releases independently of WG.
- **One install, whole stack.** The metapackage depends on the pi CLI, so
  `npm i -g @worksgood/cli` brings WG + pi (matching the README's two-step
  cargo+npm flow) and `--ignore-scripts` users keep full functionality (pi's
  own package also ships no install hooks; WG execs `pi` as a sibling CLI).
- **Publish order: platform packages first, metapackage last.** A brief
  registry-visibility window otherwise lets metapackage N resolve before
  platform package N exists.
- **Fallback = cargo hint, not nested npm.** Unsupported platform / missing
  optional dep → friendly `cargo install --git …` pointer (research doc §2.5).

## Known limitation: macOS binaries are unsigned and un-notarized

The `@worksgood/darwin-arm64` payload ships the release binaries as-is. Apple
Developer ID signing and notarization secrets are not configured yet, so
Gatekeeper may block the first run. Users can allow it with
`xattr -d com.apple.quarantine "$(which wg)"` (repeat for `worksgood` and
`nex`) or right-click → Open. Do **not** describe the macOS packages as signed
or notarized until those secrets are configured and the release pipeline
signs them.

## CI wiring (`.github/workflows/release.yml` → `npm-package` job)

The job downloads the two slice-target release artifacts, runs
`make-packages.sh --pack`, runs the resolver unit tests + `npm publish
--dry-run` on every package, uploads the tarballs as a workflow artifact, and
**publishes only when BOTH**:

1. the run is a publishable `v*` release (`plan.outputs.publish == 'true'`),
   and
2. the operator armed it: repository **variable** `NPM_PUBLISH_ENABLED=true`
   plus an npm credential (`NPM_TOKEN` secret, or npm trusted publishing via
   the job's `id-token`/registry-url OIDC setup).

Publishing uses `npm publish --provenance` (sigstore provenance linking each
tarball to the repo/commit/workflow run) and `--access public` for the scoped
packages. Dry runs and `release-test-*` tags never publish.

## Operator checklist (publish is deliberately NOT automatable from here)

The checklist is also printed by `make-packages.sh` on every run:

1. **Claim the org + names.** Create the npm org `@worksgood`; reserve
   `@worksgood/cli`, the two slice platform packages, and the phase-2 names
   (`linux-arm64-gnu`, `darwin-x64`, `win32-x64`, the two musl names) before
   first publish — a typosquatted platform-package name is the residual
   attack surface called out in the research doc §5.
2. **Set up publishing credentials.** Preferred: npm *trusted publishing*
   (link `graphwork/wg` + workflow `release.yml` per package, OIDC — no
   token). Alternative: granular automation token scoped to `@worksgood`
   only, stored as the repository secret `NPM_TOKEN`.
3. **Arm the gate.** Repository variable `NPM_PUBLISH_ENABLED=true`.
4. **Rehearse.** Push a `release-test-*` tag; confirm the `npm-package` job
   packs + dry-run-publishes; download the `npm-packages-*` artifact and
   inspect the tarballs.
5. **First real publish.** Tag `vX.Y.Z` (must equal Cargo.toml's version —
   the plan job already enforces this).
6. **Verify.** `npm audit signatures`; clean-container
   `npm i -g @worksgood/cli && wg --version && worksgood --version`;
   check the provenance link on the registry page (repo/commit/workflow run).

## Local verification story

`scripts/npm/verify-install.sh` is the runnable proof of the whole slice:

- packs all tarballs (`npm pack`),
- asserts the zero-install-scripts invariant structurally,
- installs into a **clean prefix** — proving the metapackage pulls the right
  platform package (via exact-pinned optionalDependencies) **and** the pi CLI,
- runs `wg --version` / `nex --version` / `worksgood --version` **from the
  npm-installed layout** (the shim resolved + execed the real binaries),
- repeats the install with `--ignore-scripts` and re-proves it,
- proves the `WG_BINARY_PATH` override execs the pointed-at binary (and fails
  loudly on a missing override target),
- proves the cargo-install fallback when the platform package is absent,
- runs `npm publish --dry-run` on every package (no registry write, no token).

Pinned as the smoke scenario `npm_pack_install_dry_run`
(`tests/smoke/scenarios/npm_pack_install_dry_run.sh`, loud SKIP 77 without
prebuilt binaries or registry access).
