# FS Watcher Multi-User Validation

Validation results for the inotify-based file watcher used by the TUI to detect
`.workgraph/` changes in real time across concurrent instances.

## Architecture Summary

- **Library**: `notify` v7 + `notify-debouncer-mini` v0.5
- **Backend**: inotify (Linux), FSEvents (macOS)
- **Debounce**: 50ms window via `notify-debouncer-mini`
- **Watch scope**: Recursive on `.workgraph/` directory
- **Signal mechanism**: `AtomicBool` flag (`fs_change_pending`), checked by `maybe_refresh()`
- **Write pattern**: `save_graph_inner` writes to `.graph.tmp.<pid>`, fsyncs, then `rename()` (atomic)

## Automated Test Results

Test file: `tests/integration_multi_user_watcher.rs` (7 tests, all passing)

| Test | Watchers | Writes | Interval | Result |
|------|----------|--------|----------|--------|
| `test_multi_user_watcher_spaced_writes` | 5 | 10 | 60ms | PASS - all watchers notified |
| `test_multi_user_watcher_burst_writes` | 5 | 20 | 0ms (burst) | PASS - debounce coalesces, no starvation |
| `test_multi_user_watcher_seven_users` | 7 | 15 | 30ms | PASS - all 7 watchers notified |
| `test_multi_user_watcher_single` | 1 | 5 | 60ms | PASS - baseline |
| `test_multi_user_watcher_latency` | 1 | 1 | n/a | PASS - latency < 100ms (50ms debounce + kernel) |
| `test_multi_user_watcher_inotify_capacity` | 10 | n/a | n/a | PASS - 10 recursive watchers on 6-dir tree |
| `test_multi_user_watcher_subdirectory_events` | 1 | 3 | 80ms | PASS - events from subdirs detected |

## Manual Test Procedure

To validate 5 concurrent TUI instances with rapid CLI writes:

```bash
# Terminal 1-5: launch TUI instances
wg tui  # repeat in 5 separate terminals

# Terminal 6: rapid writes
for i in $(seq 1 20); do wg log some-task "burst write $i"; done

# Observe: all 5 TUIs should update within ~100ms of each write batch
```

Expected behavior: Each TUI's `maybe_refresh()` fires within one debounce window
(50ms) of the `rename()` call. The total propagation path is:

1. `save_graph_inner` calls `rename()` (~0ms)
2. inotify delivers `IN_MOVED_TO` event (~1-5ms)
3. Debouncer batches events for 50ms window
4. Callback sets `fs_change_pending = true`
5. Next TUI tick checks flag and reloads (~0-16ms at 60fps)

**Total: ~55-75ms typical, well under 100ms target for <= 7 users.**

## inotify Watch Capacity

- Default `fs.inotify.max_user_watches`: 8192 (most distros) or higher
- Each TUI recursive watch adds ~1 watch per subdirectory in `.workgraph/`
- Typical `.workgraph/` has ~5-8 subdirectories (service, agency, agency/roles, etc.)
- 7 TUIs x 8 watches = ~56 watches << 8192 limit
- Test confirmed 10 concurrent recursive watchers on a 6-directory tree: no exhaustion

**No tuning needed** for the documented 7-user target. Systems with very large
`.workgraph/` trees (hundreds of subdirectories) or extremely low inotify limits
could potentially be affected, but this is not a realistic scenario.

## Edge Cases and Notes

1. **Debounce coalescing**: Burst writes within 50ms are coalesced into a single
   notification batch. This means the watcher fires once for multiple rapid writes,
   not once per write. This is correct behavior - the TUI reloads from disk and
   gets the latest state regardless of how many intermediate writes happened.

2. **Atomic rename detection**: inotify correctly reports `IN_MOVED_TO` for the
   `rename()` call in `save_graph_inner`. All watchers see this event. The temporary
   file write + fsync + rename pattern is safe for concurrent readers.

3. **Watcher creation failure**: The TUI silently falls back to polling if the
   watcher cannot be created (e.g., inotify unavailable). This is handled in
   `start_fs_watcher()` with the `Err(_) => {}` branch.

4. **No missed final state**: Even if intermediate writes are coalesced by the
   debouncer, the final state is always read from disk on the next refresh cycle.
   The watcher only signals "something changed" - it does not deliver the content.

## Conclusion

The existing fs watcher implementation is sound for multi-user scenarios up to
the documented 7-user target. No code changes needed. The 50ms debounce window
provides a good balance between responsiveness (<100ms end-to-end) and avoiding
excessive reloads under burst writes.
