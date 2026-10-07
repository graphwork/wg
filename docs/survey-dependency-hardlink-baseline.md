# Design + isolated proof: dependency-only immutable Cargo baseline via hardlinks

**Task:** `implement-the-dependency` — Cow survey Option B
(`.wg/survey/survey-cow-worktrees.md`, `docs/design-cow-worktrees.md`,
`docs/build-artifact-storage.md`).

**Scope of this task:** design + *isolated-instance* proof only. No production
code was changed; nothing under the shared baseline
`/home/bot/.cache/wg/build-targets/` and nothing in the live
`/home/bot/wg/.wg/` was read-for-write, relinked, moved, or deleted. The
reproducible harness is `docs/survey/dependency-hardlink-baseline-proof.sh`
and runs its own `HOME`, own `CARGO_HOME`, own `CARGO_TARGET_DIR`, and its own
throwaway checkouts under `/tmp`.

---

## 0. TL;DR and one important correction

- **The mechanism works.** A second attempt built from a *different* source
  root, over a private layer seeded by hardlinking only registry/git
  dependency artifacts, made Cargo recompile **only the workspace crate** and
  left the baseline **byte-identical**.
- **Measured win (this repo, debug):**
  - plain `cargo build`: cold **2.7 GiB** → second attempt **1.3 GiB private**
    (1.4 GiB shared) — **≈ 50 %**.
  - `cargo test --no-run` (the shape behind the 24.2 GB figure): cold
    **21 GiB** → second attempt **≈ 19.7 GiB private** (≈ 1.4 GiB shared) —
    **≈ 6–7 %**.
- **Correction to the survey's model.** The survey projects the concurrent
  peak as `deps(~12 GB, once) + N × workspace(~3 GB)` and calls Option B
  "the only option that reduces the *real* concurrent peak". Empirically
  **`target/debug/deps/` is not "dependencies"**: for a test build it is
  21 GiB, of which only **≈ 1.4 GiB** is registry/git dependency artifacts.
  The other **≈ 18.9 GiB is the workspace's own 200 test/bin executables**, which
  are inherently per-attempt. So Option B's win on the *heavy* class is ≈ 1.4 GB
  (**24.2 GB → ≈ 22.8 GB, ≈ 6 %**), while its win on the *inner-loop* class
  (`cargo build`) is ≈ 50 %. The heavy admission number is dominated by
  workspace test-executable churn, which **only culling/lifecycle bounding
  (Option E / per-attempt cull) or not building all targets can move** — not
  by hardlinking dependency artifacts.

Reproduced evidence is in §5 and the raw run in §7.

---

## 1. Mechanism: what can be shared vs what must stay private

### 1.1 Two classes of target content

Hardlinking is safe only for files that Cargo will **never rewrite for the same
build key**. Everything mutable under the same name must be a real private copy.

**Shareable (hardlink to the immutable baseline, 0 new data blocks):**

| Path under `target/<profile>/` | Why it is immutable for a matched key |
|---|---|
| `deps/lib<pkg>-<hash>.rlib` | registry/git crate output; content-addressed by package + features + profile + target + rustc + dep fingerprints |
| `deps/lib<pkg>-<hash>.rmeta` | same |
| `deps/lib<pkg>-<hash>.so` | proc-macro / dylib output; same |
| `deps/<pkg>-<hash>.d` | dep-info for a registry/git unit; only rewritten if that unit rebuilds (key prevents it) |
| `.fingerprint/<pkg>-<hash>/lib-<pkg>` | immutable fingerprint **data** (a hex fingerprint) |
| `.fingerprint/<pkg>-<hash>/dep-lib-<pkg>` | immutable dep-info |
| `.fingerprint/<pkg>-<hash>/*.json` | immutable fingerprint recipe |

**Private per attempt (real copy, `chmod u+w`):**

| Path | Why it must be private |
|---|---|
| workspace artifacts: `libworksgood-*.rlib/.rmeta/.d`, `wg-*`, `nex-*`, `casa_adapter-*`, and **all** unit/integration test executables | the worker edits workspace source; these are rebuilt/relinked every attempt |
| **all** of `build/**` (`out/`, `output`, `root-output`, `stderr`) | an arbitrary `build.rs` may rewrite its `out/` in place (survey §B; the existing `clone_regular_file_private` comment says mutable artifacts are never hard-linked) |
| `.fingerprint/**/invoked.timestamp` | Cargo touches it when it starts a unit |
| `target/<profile>/<bin>` and top-level `*.d` | rebuilt binaries |
| `.rustc_info.json`, `.cargo-lock`, `CACHEDIR.TAG` | Cargo rewrites/holds these |

### 1.2 Classification authority — a real pitfall

**Classify by the `.d` dep-info first source path, not by target name.**
The proof first tried name matching against `Cargo.toml` target names and
got it wrong twice:

1. workspace binary target `casa-adapter` appears in `deps/` as
   `casa_adapter-<hash>` (hyphen→underscore);
2. the **232 test executables are named after the test file**
   (`integration_resume-<hash>`, `test_verify_timeout_functionality-<hash>`),
   so they do not match any `Cargo.toml` target name at all and were wrongly
   classified as registry dependencies.

The robust rule that the proof settled on: for each `deps/` artifact, read its
sibling `<base>.d` and test the first source path — a path under
`$CARGO_HOME/registry/src/` (or `…/git/checkouts/…`) means registry/git
(shareable); anything else is workspace (private). The fingerprint `build/`
directories are classified by mapping their `<pkg>-<hash>` name back to that
set.

This misclassification also produced a **useful safety observation** (§4): the
wrongly-hardlinked mutable `.d` made Cargo fail loudly with
`error writing dependencies to … Permission denied (os error 13)` rather than
silently corrupting the baseline.

### 1.3 CARGO_TARGET_DIR layout + hardlink farm

Reusing the existing `docs/build-artifact-storage.md` skeleton:

```text
${cargo_target_root:-$XDG_CACHE_HOME/wg/build-targets/<project-key>}/
  baselines/<dep-key>/target/          # immutable dependency-only baseline
    READY                              # publication fence (written last)
    .wg-target-baseline.json
    target/<profile>/{deps,.fingerprint,build}/…
  layers/<dep-key>/<agent>/target/     # per-attempt writable layer
    .wg-target-layer.json
    target/<profile>/…
  locks/<dep-key>.lock
```

Composition (`prepare_layer` analogue):

1. Build the baseline cold once, then **freeze it read-only**: walk the tree and
   `chmod a-w` every directory and file (`a-w`, **not** `444`, so executable
   bits on binaries survive). Write `READY` last.
2. Create the private layer directory.
3. For each baseline file: **hardlink** it if it is shareable (§1.1 selection),
   otherwise **copy** it and `chmod u+w` the copy.
4. Preserve Cargo's own intra-target hardlinks when copying (Cargo links
   `target/<profile>/wg` ↔ `target/<profile>/deps/wg-<hash>`; copying each name
   independently would double the private footprint — the proof tracks source
   inode → first copy and hardlinks subsequent names to it).

### 1.4 How Cargo's fingerprinting tolerates the hardlink farm

- Cargo's per-unit fingerprint hash is derived from package id, features,
  profile, target triple, rustc, rustflags, and the fingerprints of its
  dependencies — **not** from the absolute workspace path or the absolute target
  directory. Therefore a registry dependency compiled in `checkout-A` is
  accepted as fresh in `checkout-B`. **Proved:** after seeding the layer from a
  baseline built in `checkout-A`, `cargo build` in `checkout-B` reported
  `Compiling worksgood` exactly once (all 400+ registry deps skipped) and
  `Finished` in 72 s.
- Output names embed a metadata hash (`libfoo-<16 hex>`). If feature
  unification or flags change, the hash changes, so Cargo targets a **new
  filename** and writes a new private file — the shared baseline entry is not
  the write target. A *same-name* rewrite is only possible when the unit is
  genuinely stale, which the build key is designed to prevent (§2); in that
  case the read-only hardlinked file makes it fail loudly (§4) rather than
  mutate the baseline.
- Hardlinks preserve mtime and permissions, so Cargo's mtime-based freshness
  comparisons see the same values as in the baseline.

---

## 2. Build key for an interactive (command-identity-less) baseline

Today `compute_key` sets `baseline_reusable = false` whenever WG does not
control an exact attested shell command, so interactive `pi` workers never
consume or publish a baseline (`docs/build-artifact-storage.md`: "A future
arbitrary Cargo shell command can never fall back to a partially keyed 'exact'
baseline"). Option B adds a **second key class** for the *dependency* portion:

- Key **without** `command_identity`: `Cargo.lock`, workspace manifests /
  toolchain files / Cargo config (`cargo_inputs`), `rustc`/`cargo --version`,
  target triple, profile, features, `CARGO_HOME` (registry source paths), and
  `working_directory` (normalized for managed worktrees).
- `dependency_baseline_reusable = true` for this class.
- A new double / field distinguishes it from the exact-command class so
  admission and promotion treat them separately.

Because the key excludes the workspace command but includes everything that
affects dependency artifacts (lockfile, features, profile, target, toolchain,
Cargo home), the dep baseline is either exactly compatible or absent.

---

## 3. Disk win, quantified

All numbers measured in the isolated instance (§5); `du` for unique physical
bytes, and private bytes as the sum of `st_blocks×512` over inodes **not**
shared with the baseline.

### 3.1 Plain `cargo build` (developer inner loop)

| | bytes | |
|---|---|---|
| cold baseline target (logical `du`) | **2.7 GiB** | |
| registry dep artifacts (shareable) | ≈ 1.4 GiB | rlib 860 MiB + rmeta 360 MiB + proc-macro `.so` 244 MiB |
| second attempt private after seed | **1.3 GiB** | workspace bins/lib + `build/` |
| second attempt private after rebuild (1 workspace crate) | **1.3 GiB** | unchanged |
| reduction per attempt | **≈ 50 %** | |

### 3.2 `cargo test --no-run` (the shape behind `build_heavy_delta`)

Measured twice: once with default debuginfo and once with the flags WG's spawn
path actually exports (`CARGO_PROFILE_DEV_DEBUG=line-tables-only`,
`CARGO_PROFILE_TEST_DEBUG=line-tables-only`, `CARGO_INCREMENTAL=0`). Both gave
identical composition, so the ratio is robust:

| | bytes | |
|---|---|---|
| cold baseline `target/debug` (`du`) | **21 GiB** | 24.2 GB high-water is this class |
| of which registry dep artifacts (shareable) | **1.40 GiB** | `deps/` `.rlib/.rmeta/.so/.d` for 410 registry packages |
| of which workspace executables (private) | **18.88 GiB** | **200** test/bin executables, 150–290 MB each |
| of which other workspace artifacts (private) | **≈ 0.3 GiB** | workspace rlib/rmeta + `build/` + fingerprints |
| second attempt private (authoritative classifier) | **19.7 GiB** | |
| reduction per attempt | **≈ 6–7 %** | |

### 3.3 Projection for the admission number

`disk_sentinel::projection_for_class` consumes
`build_high_water.build_heavy_delta_bytes = 24 192 229 376`. If deps are shared
by hardlink and `layer_bytes` charges shared inodes once:

```
new heavy delta ≈ 24.2 GB − 1.4 GiB (≈1.50 GB) ≈ 22.7 GB   (−6 %)
```

The heavy class stays dominated by per-attempt workspace test executables.
For the `build_capable` class (plain build) the projection roughly halves.
**Conclusion:** Option B is worth doing for the inner loop and for correctness
of the shared-baseline machinery, but it is **not** sufficient to stop the
heavy sentinel deferring — that needs the sibling Option E (high-water
decay/percentile) and/or bounding the per-attempt test-executable lifecycle.

---

## 4. Safety and rollback

### 4.1 If a worker writes into the shared baseline

Two distinct avenues; they have very different strength:

1. **Ordinary write through a layer hardlink.** The baseline files are
   `0444`/`0555`; Cargo/rustc opening the layer name for truncate/write gets
   `EACCES`. **Observed in the proof** when a mutable `.d` was wrongly
   hardlinked:
   `error: error writing dependencies to …/casa_adapter-….d: Permission denied (os error 13)`
   The build fails; the baseline is untouched. Fail-loud, never silent.
2. **`chmod` the layer hardlink.** A hardlink is *the same inode*, so
   `chmod u+w layer/deps/libfoo.rlib` also changes the baseline inode's mode,
   and a subsequent write **would** mutate the baseline bytes. This is the
   fundamental difference from reflinks (distinct inodes with CoW extents).
   Hardlinks cannot enforce per-name permissions.

**Mitigations for avenue 2 (required before a shared rollout):**

- **Detect.** Extend baseline publication/reuse verification with a
  per-file `(path, size, mtime, inode, mode)` manifest digest (or content
  digest for the small fingerprint-data files) checked before a layer is
  seeded and before a baseline is reused; on mismatch, discard and rebuild the
  baseline. This mirrors the existing `baseline_is_ready` /
  `BASELINE_MANIFEST` ownership fence.
- **Narrow the owner.** Run workers under a distinct UID / user namespace
  (or `chattr +i` where the operator has `CAP_LINUX_IMMUTABLE`) so the worker
  cannot `chmod` a baseline-owned inode.
- **Prefer reflinks when present.** Keep the existing `FICLONE` path as the
  first choice (distinct writable inodes) and use hardlinks only as the ext4
  fallback, exactly as `clone_regular_file_private` already falls back
  reflink→copy.

### 4.2 Rollback to today's behavior without touching live workers

- The mechanism is a **spawn-time** decision behind a config flag
  (`dependency_hardlink_baseline`, default `false`). A flag flip affects only
  *newly spawnable* attempts; already-prepared layers and running workers keep
  their current `CARGO_TARGET_DIR` and are never modified.
- Reload with `wg config reload` (no daemon restart). Existing shared baselines
  may be left in place (read-only, harmless) or reclaimed by `wg gc`.
- The fallback path is byte-for-byte today's behavior: `compute_key` sets
  `baseline_reusable = false` for interactive workers and
  `clone_regular_file_private` reflinks-or-copies into a private layer.
- The isolated proof never touched the shared cache or live `.wg`, so no
  cleanup is required to revert this task.

### 4.3 Accounting change required (small, scoped)

`layer_bytes` currently charges hardlinked inodes once **per tree** and only
treats an inode as externally linked when the tree is *unmanifested*
(`src/target_cache.rs:1296`, `validated_layer_manifest(path).is_none()`). For a
manifested Option-B layer that means shared dependency hardlinks would be
charged full blocks to every layer, defeating the projection. Option B needs
`layer_bytes` to compare each inode's `nlink` against the count found inside the
layer and charge external links **0** — the same technique already present for
unmanifested roots.

---

## 5. Isolated-instance proof

Harness: `docs/survey/dependency-hardlink-baseline-proof.sh` (own `HOME`,
`CARGO_HOME`, `CARGO_TARGET_DIR`, two `git archive` checkouts at the same HEAD,
nothing else). Raw run: §7.

Steps and observed results:

1. **Cold baseline** built in `checkout-A` → `baseline/target` = **2.7 GiB**
   (deps 2.4 GiB, build 312 MiB, .fingerprint 11 MiB), 128 s. Frozen with
   `chmod a-w`.
2. **Seed layer** from baseline (hardlink deps, copy mutable): 2838 files
   hardlinked, 2633 private inodes, **1.3 GiB private**.
3. **Rebuild in `checkout-B`** (different source root) over the layer, after a
   source mutation:
   `crates compiled: 1` → `Compiling worksgood v0.1.0 (/tmp/…/checkout-B)`,
   `Finished in 1m 11s`. All registry deps reused.
4. **Immutability:** per-file SHA-256 snapshot of the baseline before vs after
   the second attempt: `PASS: every baseline file byte-identical`.
   (`nlink>1` on 2930 baseline files is the expected hardlink farm; content
   unchanged.)
5. **Representative heavy build** performed for §3.2 and §3.3 (cold
   `cargo test --no-run`), with the same classifier.

---

## 6. Enabling it for the shared deployment — **described only, NOT applied**

This is a **separate gated step**; nothing here was executed.

**Switch.** Add `[dispatcher.resource_management] dependency_hardlink_baseline`
(default `false`, emergency-visible via `wg config lint`). Add the
`dependency_baseline_reusable` key class (§2) and a promotion path that lets a
clean checkout publish a *dependency-only* baseline even though its command
identity is `unknown-isolated`.

**Risk.**

- Avenue-2 hardlink mutation (§4.1) — hardlinks do not isolate permissions.
  Requires the detect-and-rebuild manifest verification before reuse and,
  ideally, a distinct worker UID.
- `bounded_worktree_build_storage.sh` currently asserts "every baseline/layer
  regular artifact has a distinct writable inode". Option B deliberately shares
  dependency inodes, so that scenario's invariant must be split into
  "private workspace artifacts have distinct inodes; registry dep artifacts are
  hardlinks (nlink>1) and the baseline is byte-identical".
- Feature-unification mismatch → loud `EACCES` (§4.1 avenue 1). Acceptable
  because it is detected, but it should be covered by a test so the failure
  mode is a clear diagnostic, not a mystery build break.

**Verification (when the gated step is taken).**

1. Unit tests: the classifier (registry vs workspace via `.d`),
   `layer_bytes` external-link charging, intra-target hardlink preservation.
2. A new smoke scenario (WG `tests/smoke`) that builds a baseline, seeds a
   second layer over it, and asserts: second-attempt private delta == the
   private portion; baseline content digest unchanged; a `chmod u+w` + write
   tamper is detected and the baseline rebuilt.
3. `wg disk doctor --json` before/after showing the reduced per-layer charge.
4. Enable first for `build_capable` (inner loop) where the win is ≈ 50 %;
   measure the `build_heavy` class before enabling there — the expected win is
   only ≈ 6 % and the heavy deferrals need Option E / lifecycle culling.

---

## 7. Evidence appendix

Reproducible run of the harness on this host (isolated `/tmp/wg-cow-proof-final`):

```text
=== 1. cold baseline build (checkout-A) ===
baseline build: 128s, logical size 2.7G
2.4G  …/baseline/target/debug/deps
312M  …/baseline/target/debug/build
11M   …/baseline/target/debug/.fingerprint
frozen; snapshot sha: 1a04864dca69c1729b8eef748d2647d6407faa180d4d363654120254d33a4ad0

=== 2. compose private layer over baseline (hardlink deps, copy the rest) ===
seed: shared(hardlinked) files=2838  private inodes=2633  private bytes=1338003456 (1.3G)

=== 3. real rebuild in checkout-B (different source root) over the layer ===
layer build: 72s
crates compiled: 1
   Compiling worksgood v0.1.0 (/tmp/wg-cow-proof-final/checkout-B)
    Finished `dev` profile [unoptimized + debuginfo] target(s) in 1m 11s
final: private bytes=1338011648 (1.3G)  baseline logical=2.7G
wg 0.1.0

=== 4. baseline immutability ===
PASS: every baseline file byte-identical after the second attempt
baseline shared-inode count (nlink>1, expected from hardlink farm): 2930

=== summary ===
cold baseline logical:        2.7G
second-attempt private bytes: 1.3G
hardlinked (0 new bytes) files: 2838
```

Heavy-class composition (cold `cargo test --no-run`; both default debuginfo
and the spawn flags `CARGO_PROFILE_*_DEBUG=line-tables-only`):

```text
heavy target logical (du):        21G
registry dep artifacts:           1.40 GiB  (410 registry packages)
workspace executables:            200 executables, 18.88 GiB
other workspace artifacts:        ≈0.3 GiB (rlib/rmeta + build/ + fingerprints)
heavy-layer private bytes:        21177024512 (19.72 GiB)
```

Misclassification fail-loud (before the `.d`-based classifier was adopted):

```text
error: error writing dependencies to `…/layer/target/debug/deps/casa_adapter-6879e2c5cf53ecc9.d`:
       Permission denied (os error 13)
error: could not compile `worksgood` (bin "casa-adapter") due to 1 previous error
```

---

## 8. Files

- `docs/survey/dependency-hardlink-baseline-proof.sh` — reproducible isolated
  harness.
- this file.
