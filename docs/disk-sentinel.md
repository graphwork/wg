# Disk sentinel and owned build caches

WG separates disk admission from service pause. Low space pauses only build-capable work; dot-prefixed agency tasks, read-only work, and graph operations remain eligible. Build-heavy tasks (Cargo build/test/install/clippy, CMake, and full-suite validation) also use a separate concurrency budget, serialized by default.

## Configuration

All keys live under `[dispatcher.resource_management]`:

```toml
disk_sentinel_enabled = true
disk_paths = ["/mnt/build", "/tmp"]
cargo_target_root = "/mnt/build/wg"   # optional; creates wg-target-<agent>
build_tmp_root = "/tmp/wg-build"     # optional root; default is system temp

disk_warning_bytes = 68719476736      # 64 GiB
disk_pause_build_bytes = 34359738368  # 32 GiB
disk_hard_refuse_bytes = 17179869184  # 16 GiB
disk_warning_percent = 12.0
disk_pause_build_percent = 8.0
disk_hard_refuse_percent = 4.0
disk_resume_hysteresis_bytes = 5368709120
disk_resume_hysteresis_percent = 2.0
max_build_agents = 1
estimated_build_bytes = 17179869184        # ordinary build-capable cold floor
estimated_build_heavy_bytes = 68719476736  # Cargo test/link cold floor
build_link_test_safety_bytes = 8589934592  # final link/test scratch
disk_scan_interval_seconds = 30
disk_scan_max_entries = 200000
owned_cache_lease_seconds = 300
compress_terminal_streams = true
stream_retention_days = 7
terminal_stream_max_bytes = 67108864
terminal_output_tail_bytes = 2097152
```

Both byte and percentage thresholds are enforced for the graph/project-worktree mount and every distinct configured target/tmp mount. A paused state clears only after every mount exceeds the pause threshold plus the configured hysteresis.

Build admission is projected, not just reactive. WG persists the measured per-target high-water and uses the larger of that measurement or the class-specific cold floor, then adds final-link/test safety and the unmaterialized reservation for every concurrent live build. The projection must remain above the **warning** floor. Before returning a pressure refusal, admission runs one idempotent cleanup of eligible explicitly-owned caches/terminal streams and reassesses; unknown, dirty-source, live/open and artifact guards remain unchanged. Spawn admission is serialized through the agent registry lock, so two large builds cannot both spend the same free bytes; heavy builds are additionally serialized by `max_build_agents`.

## Ownership and cleanup safety

A build-capable spawn writes `.wg/service/disk/owned-caches.json` before it is allowed to continue untracked. Each lease records the exact path, cache kind, task, agent, PID and `/proc` start identity, mount device, creation time, expiry, and owning worktree. Absolute and `/tmp` targets are first-class; cleanup never searches by a `wg-target-*` filename. Every build-capable spawn also receives an isolated `TMPDIR` (`wg-cargo-tmp-<agent>`) so Cargo-install scratch is owned and reapable rather than orphaned.

Automatic cache removal requires all recorded owners of a path to satisfy every guard:

1. the registry execution attempt is terminal and its graph task is still known;
2. the attempt lease was explicitly released or expired;
3. the exact PID identity is gone or demonstrably recycled;
4. the mount identity is unchanged;
5. the cache path contains neither the project nor its owning worktree;
6. no registered artifact is inside the path; and
7. no process has an open file, cwd, or root below the path.

**Source preservation and cache reclamation are independent.** A dirty worktree never authorizes source deletion, but it also does not pin a proven external/child build cache: Cargo can rebuild it on retry. Destructive worktree cleanup has its own stricter gate: eval passed, branch reachable **or `git cherry` patch-equivalent** to main, no live occupant, and source-clean. Only the exact *untracked root*, regular `.wg-cleanup-pending` file with WG's empty legacy payload or exact retry payload is validated lifecycle metadata and ignored by that source-clean check; arbitrary/oversized/non-file content, a tracked modification, the same basename elsewhere, or any other tracked/untracked change blocks removal. Dirty/unmerged worktrees remain visible and retryable in place.

Unknown directories, source, live/open targets, changed mounts, artifacts and inconclusive identities are preserved. Terminal `done`/`fail` attempts release their target leases promptly. Resource exhaustion is classified as `resource-exhausted-disk`: it returns the task to disk admission with its worktree preserved and does not dispatch a quality evaluator.

## Operations and observability

```sh
wg disk doctor                 # bounded refresh and human report
wg disk doctor --json          # scriptable snapshot
wg disk doctor --cached        # no scan; daemon-produced snapshot only
wg disk cleanup                # safety dry-run
wg disk cleanup --execute      # reap only proven owned stale caches
wg status                      # cached disk summary
wg status --json               # full cached snapshot under `disk`
```

The daemon refreshes a bounded cached snapshot off the TUI input/render thread. It reports mount space, target size/growth/staleness, `.wg-worktrees`, `.wg/agents`, `.wg/log`, active build counts, and projected headroom. The TUI asynchronously reads only this small cache.

Terminal raw/canonical streams are zstd-compressed after the explicit retention window **or immediately on crossing the per-file byte budget**; a bounded final JSONL tail remains at the original path so TUI structured history stays readable. Large `output.log` is retained once as `output.log.zst`; a bounded final plain-text tail remains at both `output.log` and the historical `output.txt` hard-link so `wg show`/TUI history stays readable. The full raw evidence remains decodable, while canonical task result, final assistant response, usage/cost, failure/recovery reason, session summaries, evaluation evidence, task logs and registered artifacts remain available. Live streams are never compacted.

`wg disk cleanup` reports path-level `eligible`, `reaped`, `compressed`, `deduplicated`, `ignored`, and `preserved` decisions. Re-running it is idempotent.
