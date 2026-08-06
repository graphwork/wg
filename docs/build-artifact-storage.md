# Bounded worktree build artifacts

## Physical-byte model

WG does not share a mutable Cargo target directory between divergent worktrees.
A build-capable attempt receives a private target layer under the single owned
build cache:

```text
${cargo_target_root:-$XDG_CACHE_HOME/wg/build-targets/<project-key>}/
  baselines/<cache-key>/target/       # immutable, shared lower
  layers/<cache-key>/<agent>/target/ # private writable directory tree
  locks/<cache-key>.lock
```

On ext4, unchanged regular files in a layer are hard links to read-only baseline
files. This is physical copy-on-write without relying on reflinks (the incident
host's ext4 does not support them). Cargo normally publishes outputs by rename,
which creates a private inode. An unsafe direct in-place overwrite fails because
the shared inode is read-only; it cannot silently clobber the baseline. Cargo's
`.cargo-lock` and `.rustc_info.json` are never linked. Incremental compilation is
disabled, so no mutable incremental database is shared.

A clean completed layer is promoted behind a per-key lock. `READY` is the
publication boundary; a crash before it leaves an unpublished directory which
the next promoter repairs. Dirty or differently keyed source is never promoted.
Cleanup retains every baseline referenced by an active layer, keeps one newest
inactive warm baseline, and reaps older superseded baselines. Source and
uncommitted work are outside this cache and remain governed by the stricter
worktree retention gates.

The key includes:

- Git source tree baseline
- `Cargo.lock` content
- full `rustc --version --verbose` / toolchain identity
- target triple
- declared feature set (`WG_CARGO_FEATURES`, default `default`)
- profile (`WG_CARGO_PROFILE`, default `test`)
- Rust/Rustdoc/Cargo encoded flags, incremental policy, and test debuginfo policy

Cargo fingerprints remain the second, fine-grained validation layer. Different
keys never share an immutable baseline; each writer always has its own target.

## Test-binary amplification

Cargo auto-discovery previously made every top-level `tests/*.rs` file a separate
integration crate. There were 176 files (177 independent targets in the incident
build when the then-current extra target is included). Each linked the complete
WG dependency graph and full DWARF. A representative executable was 311 MiB with
only 3.1 MiB of `.text`; roughly 300 MiB was DWARF. The observed full tree was:

| Before (incident, 2026-08-06) | Physical/logical bytes |
|---|---:|
| `target/debug/deps` | 57 GiB |
| `target/debug/incremental` | 15 GiB |
| complete target | 74 GiB |
| two isolated workers | about 148 GiB |

`autotests = false` now selects eight bounded domain harnesses, seven single-case
isolation harnesses for files which mutate process-global environment, and the
existing standalone snapshot harness (to preserve approved snapshot identity).
Original case files remain modules, so test names/filtering and existing
`serial_test` guards are preserved. Nextest is also supported for per-test
process isolation.
Routine `[profile.dev]` and `[profile.test]` use
`debug = "line-tables-only"` and `incremental = false`. A forensic full-DWARF
rerun is explicit:

```sh
cargo test --profile test-full-debug <filter>
cargo build --profile dev-full-debug
```

Measured clean representative `cargo test --tests --no-run` after consolidation
(on the same repository/toolchain, 2026-08-06):

| After | Bytes |
|---|---:|
| complete `target` | 5,603,798,140 (5.22 GiB) |
| `target/debug/deps` | 5,282,385,315 (4.92 GiB) |
| `target/debug/incremental` | 0 (directory: one 4 KiB block) |
| 16 integration executables | 2,233,181,760 (2.08 GiB) |

The representative five-case federation harness is 113 MiB rather than roughly
five 311 MiB executables. Its `.text` is about 20 MiB; line tables preserve
line-level failure locations. The complete no-run suite linked 21 total test
executables (five binary/library unit harnesses, eight integration domains,
seven necessary process-global isolation groups, and the snapshot harness), not
177.

Thus the steady-state physical model for `N` same-baseline workers is:

```text
one baseline + sum(private changed inodes per attempt) + bounded link scratch
```

not `N * complete target`. A hard-link clone has the full logical tree but only
the small layer manifest is privately allocated until Cargo replaces an output.
`wg disk doctor --json` reports both `bytes` (logical) and `private_bytes`
(physical attempt delta), plus the `cache_key`.

## Admission and lifecycle

The disk sentinel is enabled by default. Explicitly disabling it reports
`Warning`, never `Healthy`; zero free bytes always reports `HardRefuse`, including
when thresholds were misconfigured to zero. Admission runs before launch and
reserves measured physical **private deltas**, not another copy of the shared
baseline already reflected by `statvfs`. The defaults permit four bounded heavy
builds, subject to actual reserve, instead of serializing all validation.

The existing `.wg/service/disk/owned-caches.json` remains the sole lease and
cleanup authority. It owns private layer and temp paths; immutable baseline
lifecycle is integrated into the same cleanup pass. The legacy high-water schema
which counted complete per-worker target trees migrates once to the delta schema.
There is no new daemon, controller, compiler-cache path, or mutable shared target.

Focused worker validation should select the smallest domain/filter, for example:

```sh
cargo test --lib target_cache::tests
cargo test --test lifecycle_graph integration_task_lifecycle
```

The integrated full suite is linked/run once after focused checks. CI can use
`cargo nextest run` for per-test process isolation or `cargo test --tests` with
the eight domain harnesses.
