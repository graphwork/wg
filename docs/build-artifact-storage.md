# Bounded worktree build artifacts

WG never gives divergent workers one mutable Cargo target directory. A
build-capable attempt receives a private target layer under one project-keyed
owned cache:

```text
${cargo_target_root:-$XDG_CACHE_HOME/wg/build-targets/<project-key>}/
  baselines/<build-key>/target/       # immutable shared baseline
  layers/<build-key>/<agent>/target/ # per-attempt writable directory tree
  locks/<build-key>.lock
```

## Build key and physical sharing

The build key includes the committed source tree, `Cargo.lock`, workspace Cargo
manifests, Cargo configuration, Rust toolchain files, full `rustc --version
--verbose`, target triple, declared features/profile, and relevant Rust/Cargo
flags. Cargo fingerprints remain a second fine-grained validation layer. Dirty
source may consume a compatible baseline, but it is never promoted. Promotion
also requires the launch key to equal the current clean key, so a commit after a
stale build cannot publish old outputs under a new key.

On filesystems without safe native reflinks, unchanged regular files are hard
links to read-only baseline files. Cargo's lock and mutable rustc-info files are
never linked; incremental compilation is disabled. Cargo normally publishes an
output with temp-file plus rename, which creates a private inode. An unsafe
in-place write fails rather than changing the shared inode. Each attempt always
has its own writable directory tree, so concurrent Cargo lock files and output
publication are private.

A per-key file lock serializes clone/promotion. `READY` is the baseline
publication boundary; a crashed incomplete promotion is repaired under that
lock. Superseded inactive baselines are collected conservatively, every key
referenced by a retained/live layer is protected, and one inactive warm baseline
is retained.

The steady-state physical model for `N` same-key workers is:

```text
one immutable baseline + sum(private changed blocks per attempt) + link scratch
```

not `N * complete target`. `wg disk doctor --json` reports `bytes` (logical tree
bytes), `private_bytes` (uniquely allocated attempt blocks), and `cache_key` for
each registered target layer.

## Admission and migration

Predictive build admission is enabled for new configurations. Operators may set
`disk_sentinel_enabled = false` only as a visible emergency override; config
lint labels the override and prints the command that restores the safe default.
Admission runs before workspace/attempt creation and reserves measured physical
private deltas plus final-link safety. The immutable baseline is already charged
once by filesystem free-space measurement and is not reserved again per worker.

Build high-water schema 2 stores only per-attempt private deltas. Legacy schema
values measured complete duplicated trees (including historical 70–95 GiB
values); migration deliberately resets those invalid measurements rather than
blocking all future recovery. Fresh defaults reserve 96 GiB for the single first cold baseline builder, then project 4 GiB for ordinary private growth, 16 GiB for heavy private growth, and 4 GiB link/test safety. The independent
heavy-worker cap still follows `dispatcher.max_agents` unless explicitly
lowered, while the serialized physical reserve prevents concurrent candidates
from spending the same headroom.

## Lease and cleanup lifecycle

`.wg/service/disk/owned-caches.json` remains the sole attempt-layer and scratch
lease authority. `done`, `fail`, `wait`, incomplete/completion-wait, and owner
loss make the exact lease reclaimable. Cleanup requires the exact PID identity
to be gone/recycled, unchanged mount identity, no open file/cwd/root, and no
registered artifact under the cache. Missing graph tasks, deleted worktrees,
and purged registry owners are not treated as permanent activity once the PID
boundary is safe.

The daemon's retained-worktree maintenance lane performs disk cleanup before
source-worktree cleanup. It can therefore promote a clean terminal layer, reap
the exact private target and scratch paths, and compact their ownership rows.
A restart between worker exit and cleanup converges through the same idempotent
pass. Empty layer directory shells and superseded baselines are pruned only
inside the configured cache root.

Source is outside this cache. Cleanup never deletes a project or worktree and
source dirtiness is not authority to delete it. Dirty/unmerged source remains
byte-identical and follows the stricter worktree-retention rules. A registered
artifact inside an owned cache blocks that cache's removal; unknown target-like
directories are never inferred as owned.
