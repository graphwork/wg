# Usage Statistics Collection Research

**Task:** Evaluate approaches for collecting per-repo command usage statistics to enable reordering subcommands by utility in `wg --help` output.

## Context

wg has 50+ subcommands. Reordering `--help` output by actual usage would improve discoverability for users who run `wg --help` frequently. This requires tracking which commands are invoked and how often.

### Current Dependencies (Relevant)

From `Cargo.toml`:
- `serde` / `serde_json` - already available for JSON
- `libc` - available for file locking primitives
- `tokio` - async runtime (though stats collection should be sync/non-blocking)
- No SQLite dependency (matrix-lite uses `reqwest`, full matrix uses `matrix-sdk` with sqlite but that's optional)

## Options Evaluated

### Option 1: Append-Only Log File

**Implementation:** `.wg/usage.log`

```
2025-02-03T16:00:00Z list
2025-02-03T16:01:00Z done research-task
2025-02-03T16:02:00Z add "New task"
```

**Pros:**
- Simplest implementation (just append a line)
- Atomic on POSIX systems via `O_APPEND` - no locking needed for writes
- Preserves full history for debugging/analysis
- Human-readable, easy to inspect

**Cons:**
- Grows unbounded over time (could reach MB on heavy usage)
- Must parse entire file to compute counts for `--help`
- Need rotation/truncation strategy

**Performance Analysis:**
- Write: O(1) - single append
- Read (for --help): O(n) where n = number of log entries

**Concurrency:** Safe for writes (O_APPEND is atomic up to PIPE_BUF on Linux). Multiple readers during --help are fine.

---

### Option 2: Counter File Per Subcommand

**Implementation:** `.wg/stats/list.count`, `.wg/stats/done.count`, etc.

```
# .wg/stats/list.count
47
```

**Pros:**
- O(1) read per command when generating --help
- Simple format (just a number)
- Easy to manually reset a specific command

**Cons:**
- 50+ small files
- Each increment requires: acquire flock → read → increment → write → release
- File descriptor overhead
- Directory clutter

**Performance Analysis:**
- Write: O(1) with flock overhead
- Read: O(k) where k = number of subcommands (glob + read each file)

**Concurrency:** Requires `flock()` per file for safe increment. Multiple concurrent `wg` invocations could contend.

---

### Option 3: Single JSON Stats File

**Implementation:** `.wg/stats.json`

```json
{
  "version": 1,
  "counts": {
    "list": 47,
    "done": 23,
    "add": 15,
    "show": 12
  },
  "last_updated": "2025-02-03T16:05:00Z"
}
```

**Pros:**
- Single file, clean
- Easy to read/write with existing `serde_json`
- Human-readable
- Supports versioning for future schema changes

**Cons:**
- Full read-modify-write cycle for each increment
- Requires flock for safe concurrent updates
- Slightly higher write overhead than append-only

**Performance Analysis:**
- Write: O(k) where k = number of unique commands seen (serialize entire JSON)
- Read: O(1) file read + O(k) parse

**Concurrency:** Requires `flock()` for atomic read-modify-write. This is the main concern.

---

### Option 4: SQLite

**Implementation:** `.wg/stats.db`

```sql
CREATE TABLE usage (command TEXT PRIMARY KEY, count INTEGER);
```

**Pros:**
- Built-in concurrency handling (WAL mode)
- Efficient increments: `UPDATE usage SET count = count + 1 WHERE command = ?`
- Scales well to more complex analytics later

**Cons:**
- New dependency (`rusqlite` ~10KB compiled)
- Overkill for simple counters
- Binary format (not human-inspectable without tools)
- Goes against the project philosophy of minimal deps

---

## Analysis: Answering Key Questions

### How often is `wg --help` called?

Estimated: **Infrequently** (once per session at most). Users typically:
1. Run `wg --help` when learning the tool
2. Occasionally check when they forget a command name
3. Power users rarely need it

**Implication:** Read performance for --help is not critical. Even parsing a log file with 10K entries is sub-millisecond on modern hardware.

### How many concurrent `wg` commands might run?

Typical scenarios:
1. **Single user, manual:** Usually 1 at a time
2. **Agent coordination:** `wg service` spawns agents that each invoke `wg` commands. With `max_agents: 4` (default), we could see 4-8 concurrent `wg` invocations
3. **CI/scripts:** Potentially many in parallel

**Implication:** Concurrency is a real concern. File locking is needed for any read-modify-write approach.

### Should stats be per-repo or global?

**Recommendation: Both, with preference for per-repo**

- **Per-repo (`.wg/stats.json`)**: Different projects have different command profiles. A data-heavy project uses `wg analyze` more; an agent project uses `wg spawn` more.
- **Global (`~/.config/wg/stats.json`)**: Could aggregate across all repos for the user's overall usage pattern.

Start with per-repo only. Global is a nice-to-have for later.

### How to handle cold-start (no stats yet)?

**Recommendation:** Use hardcoded default ordering in code. When stats file doesn't exist or has insufficient data (< 50 total invocations), fall back to curated defaults:

```rust
const DEFAULT_ORDER: &[&str] = &[
    "list", "status", "ready", "show",     // Viewing
    "add", "done", "claim", "fail",        // Task lifecycle
    "log", "artifact", "context",          // Working
    "spawn", "agents", "service",          // Automation
    // ... rest alphabetical
];
```

---

## Recommendation: Option 3 (Single JSON Stats File)

**Chosen approach:** `.wg/stats.json` with file locking

### Rationale

1. **Simplicity:** JSON is already a first-class citizen in wg (serde_json is used everywhere)
2. **Readability:** Users can inspect/edit stats manually if needed
3. **Minimal overhead:** ~50 commands × ~20 bytes = ~1KB file
4. **Sufficient concurrency:** `flock()` is adequate for the expected concurrency levels
5. **No new dependencies:** Uses existing `serde_json` + `libc` (for flock)

### Why not the others?

- **Option 1 (log):** More complex to aggregate, needs rotation logic
- **Option 2 (per-file):** Too many files, more flock overhead
- **Option 4 (SQLite):** Overkill, adds dependency

### Implementation Sketch

```rust
use std::fs::{File, OpenOptions};
use std::os::unix::io::AsRawFd;

#[derive(Serialize, Deserialize, Default)]
struct UsageStats {
    version: u32,
    counts: HashMap<String, u64>,
}

fn increment_usage(workgraph_dir: &Path, command: &str) -> Result<()> {
    let stats_path = workgraph_dir.join("stats.json");

    // Open or create file
    let file = OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .open(&stats_path)?;

    // Acquire exclusive lock (blocks if contended)
    unsafe { libc::flock(file.as_raw_fd(), libc::LOCK_EX); }

    // Read current stats (or default)
    let mut stats: UsageStats = if file.metadata()?.len() > 0 {
        serde_json::from_reader(&file).unwrap_or_default()
    } else {
        UsageStats::default()
    };

    // Increment
    *stats.counts.entry(command.to_string()).or_insert(0) += 1;
    stats.version = 1;

    // Write back (truncate + write)
    file.set_len(0)?;
    file.seek(SeekFrom::Start(0))?;
    serde_json::to_writer_pretty(&file, &stats)?;

    // Lock released automatically when file is dropped
    Ok(())
}

fn get_command_order(workgraph_dir: &Path) -> Vec<String> {
    let stats_path = workgraph_dir.join("stats.json");

    // No locking needed for read (worst case: slightly stale data)
    let stats: UsageStats = std::fs::read_to_string(&stats_path)
        .ok()
        .and_then(|s| serde_json::from_str(&s).ok())
        .unwrap_or_default();

    let mut commands: Vec<_> = stats.counts.into_iter().collect();
    commands.sort_by(|a, b| b.1.cmp(&a.1)); // Sort by count descending
    commands.into_iter().map(|(cmd, _)| cmd).collect()
}
```

### Where to call `increment_usage`?

In `main.rs`, immediately after parsing:

```rust
fn main() -> Result<()> {
    let cli = Cli::parse();
    let workgraph_dir = cli.dir.unwrap_or_else(|| PathBuf::from(".wg"));

    // Track usage (fire-and-forget, ignore errors)
    let _ = increment_usage(&workgraph_dir, cli.command.name());

    match cli.command { ... }
}
```

The increment should:
- Not block on errors
- Handle missing .wg gracefully (skip if not initialized)
- Be fast enough to not add perceptible latency

### Future Enhancements (Not for MVP)

1. **Decay factor:** Commands used recently could have higher weight than commands used months ago
2. **Context-aware ordering:** Group by workflow (viewing, editing, automation)
3. **Global stats aggregation:** Merge per-repo stats for user-wide preferences
4. **Opt-out:** `wg config --no-usage-stats` for privacy-conscious users

---

## File Format Specification

```json
{
  "version": 1,
  "counts": {
    "list": 47,
    "done": 23,
    "add": 15,
    "show": 12,
    "ready": 10,
    "claim": 8,
    "log": 7,
    "status": 6,
    "spawn": 5,
    "agents": 4
  }
}
```

**Fields:**
- `version`: Schema version for future migrations
- `counts`: Map of command name → invocation count

**File location:** `.wg/stats.json`

**Concurrency:** Exclusive flock during write, shared flock (or no lock) during read.

---

## Summary

| Approach | Complexity | Concurrency | Dependencies | Recommendation |
|----------|------------|-------------|--------------|----------------|
| Append log | Low | Good (O_APPEND) | None | Not chosen - aggregation overhead |
| Per-file counters | Medium | Medium (many flocks) | None | Not chosen - too many files |
| **Single JSON** | Low | Good (flock) | None | **Recommended** |
| SQLite | High | Excellent | rusqlite | Not chosen - overkill |

**Recommendation:** Implement Option 3 (single JSON stats file) with `flock()` for concurrency safety. It balances simplicity, performance, and maintainability for wg's use case.
