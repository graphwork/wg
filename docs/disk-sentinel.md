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
estimated_build_bytes = 17179869184
disk_scan_interval_seconds = 30
disk_scan_max_entries = 200000
owned_cache_lease_seconds = 300
compress_terminal_streams = true
stream_retention_days = 7
```

Both byte and percentage thresholds are enforced for the graph/project-worktree mount and every distinct configured target/tmp mount. A paused state clears only after every mount exceeds the pause threshold plus the configured hysteresis.

## Ownership and cleanup safety

A build-capable spawn writes `.wg/service/disk/owned-caches.json` before it is allowed to continue untracked. Each lease records the exact path, cache kind, task, agent, PID and `/proc` start identity, mount device, creation time, expiry, and owning worktree. Absolute and `/tmp` targets are first-class; cleanup never searches by a `wg-target-*` filename. Every build-capable spawn also receives an isolated `TMPDIR` (`wg-cargo-tmp-<agent>`) so Cargo-install scratch is owned and reapable rather than orphaned.

Automatic removal requires all recorded owners of a path to satisfy every guard:

1. registry owner and graph task are terminal;
2. the lease expired;
3. the exact PID identity is gone or demonstrably recycled;
4. the mount identity is unchanged;
5. the owning worktree has no uncommitted source;
6. no registered artifact is inside the path; and
7. no process has an open file below the path.

Unknown directories, worktrees, source, active targets, and inconclusive process identities are preserved. The periodic sentinel does not remove worktrees.

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

After the explicit retention window, terminal raw/canonical streams are zstd-compressed. Duplicate `output.log`/historical `output.txt` pairs are replaced with hard links, preserving both readable paths. Session summaries, evaluation evidence, task logs, and registered artifacts are never compressed or removed by this policy; doctor/cleanup reports measured savings.
