# npm Distribution for the WG Rust Binaries — Decision Doc

**Task:** `research-npm-distribution`
**Date:** 2026-07-19
**Status:** Research + recommendation (no code changed)
**Related:** `docs/research/low-friction-install-upgrade.md` (the installer-script / GitHub-Release
path; this doc is the *npm* leg of that same distribution stack), `Cargo.toml`
(`[package.metadata.binstall]`), `.github/workflows/release.yml`

## Summary / Recommendation (TL;DR)

Adopt the **esbuild/biome model**: one small metapackage (`@worksgood/cli`) plus one
platform package per release target (`@worksgood/linux-x64-gnu`, `@worksgood/darwin-arm64`,
…), each platform package containing the prebuilt binaries, wired together with
**`optionalDependencies`** and resolved by a **tiny pure-JS `bin` shim with ZERO
postinstall scripts**. `cargo install --git` stays the documented primary path; npm is
**additive**. Versioning is single-source from `Cargo.toml`; all packages in a release
share one exact version.

**First slice:** linux-x64-gnu + darwin-arm64 only, packaged from the *existing*
`release.yml` artifacts (they are already built, signed, notarized, and attested —
the npm leg is pure packaging, not a new build pipeline). Estimated **2–4 working
days** for the slice; +1–2 days to cover the full five-target matrix.

## Verified vs Assumed (evidence ledger)

Verified against live sources during this research (network was available):

- **esbuild**: `lib/npm/node-platform.ts` (the OS/arch → `@esbuild/<platform>` map,
  including separate musl handling and `isValidBinaryPath` guarding
  `ESBUILD_BINARY_PATH`); `lib/npm/node-install.ts` (the fallback
  `installUsingNPM` path that runs a nested `npm install` when the optional dep is
  absent); PR #1621 ("install using optionalDependencies") which documents the move
  *away* from postinstall-download toward optionalDependencies for compatibility with
  `--ignore-scripts`, offline installs, and read-only filesystems; esbuild issue #4288
  documenting that the flow works with `--ignore-scripts` **or** `--no-optional` (not both).
- **biome**: `packages/@biomejs/biome/bin/biome` is a small JS proxy mapping
  `PLATFORMS = { win32: {x64…}, darwin: {…}, linux: {…}, "linux-musl": {…} }` →
  `@biomejs/cli-<platform>/biome`, honoring a `BIOME_BINARY` env override, with musl
  detection at runtime (`isMusl()`); the metapackage declares the eight
  `@biomejs/cli-*` packages as `optionalDependencies` (pnpm-lock/DeepWiki confirmed).
- **cargo-dist** (axodotdev): has a first-class **npm installer** backend
  (`src/backend/installer/npm.rs`, PR #210) generating exactly this shape —
  platform packages + shim, "as npm packages without postinstall scripts"; the
  independent `cargo-npm` tool (abemedia/cargo-npm) does the same ("Package and
  distribute Rust CLI binaries as npm packages without postinstall scripts").
  esbuild PR #1621 is cited by cargo-dist issue #100 as the pioneer of the pattern.
- **npm provenance**: `npm publish --provenance` (npm ≥ 9.5.0) produces sigstore-signed
  provenance + publish attestations linking the tarball to the GitHub repo/commit/
  workflow run; GitHub's announcement confirms it is the intended supply-chain
  mechanism for packages built on Actions.
- **This repo (read locally)**: `Cargo.toml` defines 4 bin targets (`wg`, `nex`,
  `worksgood`, `casa-adapter`) plus `cargo-binstall` metadata pointing at
  `{repo}/releases/download/v{version}/wg-v{version}-{target}.tar.gz`;
  `.github/workflows/release.yml` builds a 5-target matrix
  (`x86_64-unknown-linux-gnu`, `aarch64-unknown-linux-gnu`, `x86_64-apple-darwin`,
  `aarch64-apple-darwin`, `x86_64-pc-windows-msvc`), signs + notarizes macOS binaries
  (Developer ID → notarytool → stapler, before archiving), signs Windows with a PFX,
  and emits GitHub artifact attestations (`actions/attest@v4`, `gh attestation verify`).
  `README.md` documents `cargo install --git …` and users already run
  `npm install -g --ignore-scripts @earendil-works/pi-coding-agent` (the pi CLI).

Assumed (not independently re-verified; low risk, called out where it matters):

- That npm's `os`/`cpu` fields plus `optionalDependencies` correctly skip
  non-matching platform packages on every package manager in wide use (npm ≥ 7, pnpm,
  yarn 3+, bun). This is the pattern esbuild/biome/rolldown all rely on in production;
  assumed safe. One genuine gap: **glibc-vs-musl is not expressible in `os`/`cpu`** —
  musl must be a differently-named package with runtime detection (see §2).
- That exact-version pinning of platform deps inside the metapackage (no `^`) is the
  right call — this is what esbuild does; assumed.
- Effort estimates are judgment, not measurement.

---

## 1. Cargo build/release → per-target npm packages

**Key finding: WG's release pipeline already does the hard part.** `release.yml` builds
`--bins` for all five targets, handles macOS Developer ID signing + notarytool +
staple *before* archiving, signs Windows, and attaches GitHub artifact attestations.
The npm packages are therefore **pure consumers of already-produced artifacts** — no
new build matrix, no new signing work. The shape:

- **One platform npm package per release target.** Naming follows biome's convention
  (scoped under the WG org, `<os>-<arch>[-libc]`):

  | Rust target | npm package | Notes |
  | --- | --- | --- |
  | `x86_64-unknown-linux-gnu` | `@worksgood/linux-x64-gnu` | glibc; first slice |
  | `aarch64-apple-darwin` | `@worksgood/darwin-arm64` | signed+notarized; first slice |
  | `aarch64-unknown-linux-gnu` | `@worksgood/linux-arm64-gnu` | phase 2 |
  | `x86_64-apple-darwin` | `@worksgood/darwin-x64` | phase 2 |
  | `x86_64-pc-windows-msvc` | `@worksgood/win32-x64` | phase 2 |
  | (future) `x86_64/aarch64-unknown-linux-musl` | `@worksgood/linux-x64-musl` / `-arm64-musl` | separate names, biome-style; see §2 |

- **Package contents**: unpack the existing
  `wg-v<version>-<target>.tar.gz` (`.zip` on Windows) into
  `package/bin/{wg,nex,worksgood,casa-adapter}` and ship the directory as the npm
  tarball. The macOS binaries inside are already signed/notarized; codesigning covers
  the embedded `include_dir!` pi-plugin bytes (verified in `release.yml` comments), so
  repackaging does not invalidate anything. **Decision point:** ship all four bins
  (parity with the cargo install) vs only `wg`/`nex`/`worksgood` (what a human uses).
  Recommendation: ship all four for parity — the tarball is already built and size is
  not a differentiator; revisit if the archive gets heavy.
- **Platform `package.json`**: `os` + `cpu` fields so the package manager skips
  non-matching installs (`os: ["linux"]`, `cpu: ["x64"]`, …), `libc` is *not*
  standard for glibc naming — encode glibc in the package *name* (`-gnu` suffix,
  matching Rust target naming) and keep musl as separately-named packages. Each
  platform package is version-locked to the release and contains **no scripts**
  (`"scripts": {}`), no dependencies.
- **glibc vs musl**: WG currently ships glibc-only Linux. npm cannot express libc in
  `os`/`cpu`, so every mainstream Rust-via-npm project (biome, rolldown) ships musl as
  separately-named packages and detects at runtime. For WG this is a **phase-2+
  concern**: add `*-musl` packages only if users actually report Alpine/void/musl
  need; the shim's detection logic (`process.report.getReport().header.glibcVersionRuntime`
  or `ldd --version` probing, the biome approach) is designed in §2 from day one so
  adding musl later is additive.
- **macOS codesigning/notarization**: already done in `release.yml` before archiving;
  npm packaging simply re-tars the same files. One caveat worth verifying in the first
  slice: npm tarballs are gzip archives — confirm Gatekeeper accepts a notarized binary
  extracted from an npm tarball the same as from the GitHub archive (it should; the
  signature and ticket ride *inside* the binary/staple, not the outer archive).
- **CI shape**: add a `npm-package` job to `release.yml` that (a) downloads the
  assembled release archives, (b) runs a small packaging script
  (`scripts/npm/make-packages.sh`, ~100 lines: unpack, emit `package.json` per
  target, `npm publish --provenance` each), gated on the existing dry-run/
  `release-test-*` logic. Publishing order: platform packages first, metapackage last
  (it must not reference unpublished versions).

## 2. The npm wrapper: resolution, exec, fallback

The metapackage `@worksgood/cli` contains **no native code and no install scripts**.
It ships one small CommonJS/ESM shim declared in `bin`:

```jsonc
// @worksgood/cli package.json (sketch)
{
  "name": "@worksgood/cli",
  "version": "0.1.0",
  "bin": { "wg": "./bin/wg.js", "nex": "./bin/nex.js", "worksgood": "./bin/worksgood.js" },
  "optionalDependencies": {
    "@worksgood/linux-x64-gnu": "0.1.0",     // EXACT pins, no ^ (esbuild pattern)
    "@worksgood/darwin-arm64": "0.1.0"
  },
  "engines": { "node": ">=18" },
  "scripts": {}   // deliberately empty — zero postinstall
}
```

The shim (the biome `bin/biome` model, ~40 lines):

1. Resolve the platform package name from `process.platform`/`process.arch`
   (a static map like biome's `PLATFORMS`), with **musl detection** on linux
   (`glibcVersionRuntime` in `process.report` → `-gnu`, else `-musl`).
2. `require.resolve('@worksgood/<platform>/bin/wg')` — the package manager has
   already installed exactly the right optional dependency into `node_modules`.
3. Honor an escape hatch env var (`WG_BINARY_PATH`, mirroring
   `ESBUILD_BINARY_PATH`/`BIOME_BINARY`) for distro-packaged or self-built binaries.
4. Spawn the real binary with `stdio: 'inherit'` and forward the exit code. The shim
   adds ~5–15 ms of Node startup before exec — irrelevant for WG's long-running CLI
   usage (and identical to what biome/esbuild accept).
5. **Fallback on any resolution failure** (package missing because of
   `--no-optional`, `--ignore-scripts` on old npm, unsupported platform): print a
   friendly pointer instead of a stack trace:

   ```
   @worksgood/cli: no prebuilt binary for linux-musl-arm64.
   Install the Rust toolchain build instead:
     cargo install --git https://github.com/graphwork/wg --locked
   ```

   This is the one place WG improves on esbuild (whose fallback is a nested
   `npm install` download — an approach PR #1621 kept only as a legacy path). For a
   tool like WG, a clear "use cargo install" hint is simpler and safer than
   npm-inside-npm.

Why **optionalDependencies** and not the old postinstall-download approach: esbuild's
PR #1621 documents the failure modes of postinstall (broken under `--ignore-scripts`,
offline installs, custom registries/proxies, read-only filesystems) and the
optionalDependencies model fixes all of them — npm itself picks the right package.
This is also why the requirement is **zero postinstall scripts**, which conveniently
aligns with the security posture in §5.

## 3. Interaction with the existing install path

- **`cargo install --git` stays supported and stays the documented primary path.**
  Nothing in the npm leg touches the cargo path, the binstall metadata, or the
  future installer script from `low-friction-install-upgrade.md`. npm is the
  *additional* channel for users who already live in Node-land — which is precisely
  WG's core audience: the README's own flow has users run
  `npm install -g --ignore-scripts @earendil-works/pi-coding-agent` right after
  installing WG. An `npm i -g @worksgood/cli` (or `npx @worksgood/cli`) one-liner
  meets those users where they already are, with the same trust root (npm registry)
  they just used for pi.
- **The pi npm package is unaffected.** `@worksgood/pi` is a pi extension package with
  peer deps on `@earendil-works/pi-coding-agent`; it execs `wg`/`pi-handler` by
  whatever `wg` is on PATH. Whether `wg` arrived via cargo or npm is invisible to it.
  The `wg pi-plugin`/`wg pi-handler` hermetic machinery reads the *binary's* embedded
  plugin, not the install channel.
- **PATH collisions**: `npm i -g` puts shims in npm's global bin dir (e.g.
  `~/.npm-global/bin` or nvm's dir); cargo puts binaries in `~/.cargo/bin`. If a user
  installs both, whichever dir comes first in `PATH` wins. The shim should detect
  version skew (run `wg --version`-compatible check at spawn, or simply document it)
  and `wg doctor` should report which install source it resolved from — worth a small
  follow-up task (`wg doctor`/`dev-check` npm-source detection), not a blocker.
- **`wg upgrade`** (from the low-friction doc) should learn to detect an npm-owned
  `wg` and say `npm update -g @worksgood/cli` instead of replacing the binary — same
  delegate-to-package-manager rule that doc already establishes for brew/apt.

## 4. Versioning and sync

- **`Cargo.toml`'s `version` is the single source of truth.** `release.yml` already
  derives the release tag/version from it (`sed -n 's/^version = …'`). The npm
  packaging job reads the same value; all six packages (metapackage + N platform
  packages) in a release carry **identical versions**. No independent npm semver
  timeline — an npm version that doesn't match a GitHub release tag should be
  impossible by construction.
- **Metapackage pins platform deps exactly** (`"0.1.0"`, no caret). esbuild does
  this; a `^` range would let npm mix a new metapackage with an old (or future)
  platform package across an upgraded-registry race, and WG's binaries talk
  versioned graph protocols (`WG_AGENCY_COMPAT_VERSION` etc.) where skew is a real
  failure mode. npm's atomic per-package publish means a brief window exists where
  the metapackage of version N is visible before platform package N; publish platform
  packages **first**.
- **Pre-releases / release-test tags**: `release.yml` already distinguishes
  `release-test-*`/`dry-run` tags from publishable `v*` tags. npm pre-releases use
  `0.1.0-rc.1` semver prerelease form; dry runs skip publishing entirely.
- **rust-version floor ↔ engines floor**: WG's `rust-version = 1.85` is irrelevant to
  npm users (they get prebuilts); the shim's `engines.node: ">=18"` matches the
  `@worksgood/pi` dev tooling floor. No coupling between the two.

## 5. Security posture

- **Zero postinstall scripts — non-negotiable and structurally guaranteed.** The
  metapackage's `scripts` is empty and the platform packages are pure payload. This
  eliminates the entire install-script supply-chain attack class (the classic
  npm-malware vector) for WG packages, and is what esbuild PR #1621, biome, and
  cargo-dist's npm installer all converged on. It also means `npm i -g --ignore-scripts`
  users (as the README's pi step already is) get full functionality.
- **Tarball integrity**: npm computes and enforces SHA-512 integrity on every
  install automatically — the registry's `integrity` field in the lockfile pins exact
  bytes. No additional checksum file is needed *for the npm path* (unlike the
  GitHub-release path, where the installer script must check `sha256sums` itself).
- **Provenance/attestation**: publish with `npm publish --provenance` from the
  existing `release.yml` (it already holds `id-token: write` and `attestations:
  write` for `actions/attest@v4`). This attaches sigstore-signed provenance linking
  each npm tarball to the repo/commit/workflow run, and npm's publish attestation is
  verifiable by consumers and supply-chain scanners. Combined with the existing
  GitHub artifact attestations on the underlying binaries, there are two independent,
  verifiable chains: npm provenance (tarball → workflow run → commit) and GitHub
  artifact attestation (binary → workflow run → commit).
- **Trusted publishing**: configure the npm publisher as a trusted publisher /
  granular `--access public` automation token scoped to the `@worksgood` packages
  only; no long-lived personal token in secrets.
- **macOS/Windows signing** rides inside the binaries (done pre-archive, §1), so the
  npm path inherits Gatekeeper/SmartScreen posture unchanged.
- **Residual risk, honestly stated**: npm package *names* under `@worksgood` are the
  attack surface — a typosquatted or pre-empted platform-package name would be a
  problem. Mitigation: register all planned platform package names at once when the
  first slice publishes (they're cheap), and keep exact version pins in the
  metapackage so a rogue "latest" platform package can't be pulled by name alone.

## 6. Effort estimate, recommendation, and the first slice

**Effort estimate (engineering days, one worker):**

| Item | Estimate |
| --- | --- |
| Packaging script (`scripts/npm/make-packages.*`) + metapackage shim (`bin/*.js`, musl detection, fallback hint, tests of the resolution map) | 1–1.5 d |
| `release.yml` `npm-package` job wired to existing artifacts, dry-run gating, provenance | 0.5–1 d |
| npm org setup, trusted publisher, registering platform names | 0.25 d |
| End-to-end rehearsal on a `release-test-*` tag + install-path smoke (`npm i -g` in a clean container, `wg --version`, spawn pi-handler, fallback message) | 0.5–1 d |
| **First slice total (linux-x64-gnu + darwin-arm64)** | **2–4 d** |
| Phase 2: remaining three targets + doc updates (README, low-friction doc cross-ref, `wg upgrade`/`doctor` npm detection) | 1–2 d |

**Recommendation: do it — adopt the esbuild/biome optionalDependencies model, no
postinstall, `cargo install --git` stays primary.** Rationale: WG's audience already
installs pi via npm; WG's release pipeline already produces signed, attested,
per-target binaries, so the npm leg is cheap packaging on top of sunk cost; and the
pattern is thoroughly de-risked by esbuild (2021), biome, rolldown, and cargo-dist's
first-class npm installer. The main WG-specific deltas from the textbook pattern are
small: four binaries per package instead of one, glibc-in-the-name (musl deferred),
and a cargo-install hint as the fallback instead of esbuild's nested-npm download.

**Milestone M1 — first slice (concrete scope):**

1. `scripts/npm/` packaging script + `@worksgood/cli` metapackage shim (resolution
   map, `WG_BINARY_PATH` escape hatch, musl detection stub, cargo-hint fallback) with
   unit tests for the platform map and fallback paths.
2. Platform packages `@worksgood/linux-x64-gnu` and `@worksgood/darwin-arm64`,
   assembled from existing `release.yml` archives (binaries already signed/notarized).
3. `release.yml`: `npm-package` job, platform-first publish order, `--provenance`,
   gated on the existing dry-run/`release-test-*` machinery.
4. Validation: on a `release-test-*` tag — `npm pack` of all packages, install in a
   clean container (linux-x64) and on an arm64 macOS runner (`npm i -g @worksgood/cli
   && wg --version && wg init` in a temp dir), verify zero install scripts ran
   (`--ignore-scripts` install also works), verify the fallback message on an
   unsupported simulated platform, and `npm audit signatures`/attestation verify.
   Explicitly out of scope for M1: linux-arm64, darwin-x64, windows, musl, `wg
   upgrade` npm-delegation, README rewrite (phase 2).

**Open decision points** (small, resolve at M1 start): (a) ship `casa-adapter` in
platform packages or the three user-facing bins only; (b) npm package scope name —
`@worksgood/cli` assumed here, alternates are `worksgood` (unscoped, squatting risk)
or `@graphwork/wg` matching the GitHub org; (c) whether M1 publishes publicly or
holds packages private until all five targets exist (recommendation: publish the
slice publicly; partial platform coverage with a good fallback hint is exactly the
state biome shipped in for a long time).
