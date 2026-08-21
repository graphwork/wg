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
flags. Its command namespace has an explicit honesty boundary: WG accepts only a tiny
shell grammar consisting of leading literal environment assignments, one
build-like Cargo invocation, and (for the overlap probe) an optional inert
bounded `sleep`. The exact command, logical working directory, effective
`CARGO_HOME`, rustup/toolchain selector, every accepted environment assignment,
profile/release, target, features, rustflags and `--config` inputs are keyed.
`export`, `cd`, `env`, functions/subshells, redirections, pipelines, arbitrary
compound commands and dynamic expansion all fail closed. So do interactive
agents: each receives an attempt-isolated, non-reusable layer that can neither
consume nor publish a baseline. A future arbitrary Cargo shell command can
never fall back to a partially keyed “exact” baseline.

Cargo fingerprints remain a second fine-grained validation layer. Dirty source
may consume a compatible exact baseline, but it is never promoted. Promotion
also requires the launch key to equal the current clean key, so a commit after a
stale build cannot publish old outputs under a new key.

Unchanged regular files are verified native reflinks (`FICLONE` on Linux) when
the filesystem supports them. Every clone has a distinct writable inode; an
in-place truncate or overwrite triggers filesystem CoW and cannot mutate the
baseline or a sibling. If reflink capability is absent, WG makes a private byte
copy. If even private copying cannot be verified, the partial seed is discarded
and the attempt starts with an empty target. Mutable target artifacts are never
hard-linked. Cargo's lock and mutable rustc-info files are never seeded;
incremental compilation is disabled. Internal relative symlinks are recreated
as private directory entries, while absolute/escaping links are omitted so they
cannot point back into a baseline or another layer.

A per-key file lock serializes clone/promotion. `READY` is the baseline
publication boundary; a crashed incomplete promotion is repaired under that
lock. Superseded inactive baselines are collected conservatively, every key
referenced by a retained/live layer is protected, and one inactive warm baseline
is retained.

The steady-state physical model for `N` same-key workers is:

```text
one immutable baseline + sum(private changed CoW blocks per attempt) + metadata
```

not `N * complete target` on a reflink-capable filesystem. `wg disk doctor
--json` reports `bytes` (logical tree bytes), `private_bytes` (a conservative
per-inode physical charge), and `cache_key` for each registered target layer.
Because portable inode metadata cannot reveal shared CoW extents,
`private_bytes` intentionally overcharges reflinked blocks rather than
under-reserving disk. On fallback filesystems the model is safe but less
deduplicated.

## Candidate-smoke provenance

The bounded-storage smoke does not trust an inherited `target/debug/wg`. It
requires a clean submitted `HEAD`, records its commit and tree, runs `cargo
build --locked --bin wg` from that checkout, and writes a candidate-build
receipt containing the source root/commit/tree, exact executable path, build
argv and SHA-256 digest. A verifier binds all receipt fields to the live source
and bytes before invocation. The scenario deliberately substitutes a stale
executable at the expected path and requires verification to reject it before
restoring and invoking the receipt-bound binary. The canonical receipt and its
digest are emitted in the scenario's final stdout, so WG's host-captured
deterministic validation envelope binds the source-to-binary proof into
completion evidence.

## Admission and migration

Predictive build admission is enabled for new configurations. Operators may set
`disk_sentinel_enabled = false` only as a visible emergency override; config
lint labels the override and prints the command that restores the safe default.
Admission runs before workspace/attempt creation and reserves measured physical
private deltas plus final-link safety. Every build-capable class (not only tasks
classified build-heavy) reserves and serializes creation of a missing cold
baseline. Once published, the immutable baseline is already charged by
filesystem free-space measurement and is not reserved again per worker.

Build high-water schema 2 stores only per-attempt private deltas. Legacy schema
values measured complete duplicated trees (including historical 70–95 GiB
values); migration deliberately resets those invalid measurements rather than
blocking all future recovery. Fresh defaults reserve 96 GiB for the single first cold baseline builder, then project 4 GiB for ordinary private growth, 16 GiB for heavy private growth, and 4 GiB link/test safety. The independent
heavy-worker cap still follows `dispatcher.max_agents` unless explicitly
lowered, while the serialized physical reserve prevents concurrent candidates
from spending the same headroom.

## Lease and cleanup lifecycle

`.wg/service/disk/owned-caches.json` remains the sole attempt-layer and scratch
lease authority. Target and scratch rollback guards are installed at the exact
leaf-creation boundary. Each guard captures the parent and leaf filesystem
identity and writes a cryptographically random ownership marker. Before Drop
cleanup it re-opens/revalidates both identities and the marker, atomically moves
the leaf to an unpredictable quarantine name, and validates again immediately
before removal. A path replacement or copied marker therefore leaks safely
instead of deleting a pre-existing sentinel tree. A failed scratch preparation
reaps a genuine owned target but never adopts or removes a pre-existing path.
Configured absolute scratch roots are project-keyed before the agent ID, so two
graphs cannot collide merely because both allocated `agent-1`. `done`, `fail`, `wait`, incomplete/completion-wait, and owner
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
