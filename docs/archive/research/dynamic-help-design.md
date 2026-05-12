# Dynamic Help Ordering Design

**Task:** Design how `wg --help` and `wg` (no args) should use collected usage statistics to reorder subcommands.

**Dependency:** [Usage Stats Research](./usage-stats-research.md) — recommends `.wg/stats.json` with JSON counters.

---

## 1. Display Format

### Recommendation: Tiered Grouping

Group commands into three tiers based on usage frequency, with the most-used commands shown first.

```
wg - wg task management

Commands (by usage):
  list        List tasks with filters
  ready       Show tasks ready to start
  done        Mark task as complete
  show        Display task details

Less common:
  add         Create a new task
  claim       Claim a task for yourself
  log         Add progress entry
  fail        Mark task as failed

More commands:
  spawn       Start an agent
  analyze     Run dependency analysis
  ... (15 more, use --help-all)
```

### Why Tiered Grouping?

1. **Cognitive load:** A flat list of 50+ commands overwhelms users. Grouping focuses attention.
2. **Quick scanning:** Users scanning for a frequently-used command find it immediately.
3. **Learning aid:** "Less common" tier helps users discover commands they haven't tried.
4. **Conciseness:** Truncating the third tier with `--help-all` keeps default output manageable.

### Tier Thresholds

```rust
fn tier_for_count(count: u64, total: u64) -> Tier {
    let pct = (count as f64 / total as f64) * 100.0;
    match pct {
        p if p >= 10.0 => Tier::Frequent,    // Top commands (≥10% of usage)
        p if p >= 2.0  => Tier::Occasional,  // Medium usage (2-10%)
        _              => Tier::Rare,        // Everything else
    }
}
```

- **Frequent:** Commands making up ≥10% of total usage each
- **Occasional:** Commands making up 2-10% of total usage
- **Rare:** Commands used <2% of the time

These thresholds ensure:
- Typically 3-6 commands in "Frequent" (the core workflow)
- 5-15 commands in "Occasional" (useful but not daily)
- Everything else in "Rare" (specialized/advanced)

### Within-Tier Ordering

Within each tier, sort by count descending. Commands with equal counts are sorted alphabetically for determinism.

### Don't Show Counts

Showing raw counts (`list (47)`) adds noise without value. Users care about relative ordering, not absolute numbers. The tiered display already communicates relative frequency.

---

## 2. Fallback Ordering (Cold Start)

When no stats exist or data is insufficient, use a curated default ordering based on typical workflows.

### Definition of "Insufficient Data"

```rust
const MIN_TOTAL_INVOCATIONS: u64 = 20;

fn has_sufficient_data(stats: &UsageStats) -> bool {
    stats.counts.values().sum::<u64>() >= MIN_TOTAL_INVOCATIONS
}
```

Twenty invocations provides enough signal to start showing personalized ordering. Below that, the sample size is too small.

### Default Ordering

```rust
const DEFAULT_ORDER: &[&str] = &[
    // Tier 1: Essential viewing (what you run first)
    "list", "ready", "status", "show",

    // Tier 2: Task lifecycle (common workflow)
    "add", "done", "claim", "fail", "edit",

    // Tier 3: Working on tasks
    "log", "artifact", "context", "submit",

    // Tier 4: Setup & structure
    "init", "relate", "depend",

    // Tier 5: Automation
    "spawn", "agents", "service", "dispatch",

    // Tier 6: Advanced
    "analyze", "validate", "sync", "config",
];
```

Commands not in this list appear alphabetically at the end.

### Cold Start Display

When using defaults, show a flat list (no tiers) since we don't have data to justify groupings:

```
wg - wg task management

Commands:
  list        List tasks with filters
  ready       Show tasks ready to start
  status      Show project status
  show        Display task details
  add         Create a new task
  done        Mark task as complete
  ...
```

---

## 3. Staleness and Decay

### Recommendation: No Decay (Raw Counts)

**Don't implement time-based decay for MVP.**

Rationale:
1. **Simplicity:** Decay adds complexity (timestamps per command, decay algorithm tuning)
2. **User expectation:** Most CLI tools don't decay usage stats
3. **Workflow stability:** A project's command profile is usually stable over its lifetime
4. **Manual reset:** Users can delete `.wg/stats.json` to reset if desired

### Future Consideration: Last-N Window

If decay becomes needed, a simpler approach than exponential decay:

```json
{
  "version": 2,
  "window_counts": {
    "list": [12, 15, 10, 8],  // Last 4 weeks
    "done": [5, 8, 7, 3]
  }
}
```

Sum the window for display ordering. Old weeks drop off. This is conceptually simpler than exponential decay and easier to reason about.

**Not implementing for MVP** — only if users report issues with stale ordering.

---

## 4. Scope: Per-Repo vs Global

### Recommendation: Per-Repo Only (for MVP)

**Use only per-repo stats stored in `.wg/stats.json`.**

Rationale:
1. **Project personality:** Different projects genuinely have different command profiles
   - A documentation project: heavy `wg list`, `wg done`
   - An automation project: heavy `wg spawn`, `wg agents`
2. **Privacy:** Stats stay with the project, don't leak to global config
3. **Portability:** Project stats travel with the repo (if committed)
4. **Simplicity:** One stats file, one source of truth

### Future: Global Fallback

Later enhancement: when per-repo stats are insufficient AND `~/.config/wg/stats.json` exists, blend them:

```rust
fn get_effective_counts(repo_stats: &Stats, global_stats: &Stats) -> HashMap<String, u64> {
    if has_sufficient_data(repo_stats) {
        return repo_stats.counts.clone();
    }

    // Blend: repo stats weighted 3x, global as fallback
    let mut counts = global_stats.counts.clone();
    for (cmd, count) in &repo_stats.counts {
        *counts.entry(cmd.clone()).or_insert(0) += count * 3;
    }
    counts
}
```

**Not implementing for MVP** — adds complexity, unclear value.

---

## 5. Opt-Out

### Recommendation: Config Option + Flag

Two ways to disable dynamic ordering:

#### 1. Persistent Config

```bash
wg config set help.ordering alphabetical
```

Stored in `.wg/config.toml`:

```toml
[help]
ordering = "alphabetical"  # or "usage" (default)
```

#### 2. One-Time Flag

```bash
wg --help --alphabetical
# or short form:
wg --help -a
```

This overrides the config for a single invocation.

### Additional Flag: Full Help

```bash
wg --help-all
```

Shows all commands without truncation, still respecting ordering preference.

### Config Values

| Value | Behavior |
|-------|----------|
| `usage` (default) | Tiered display based on stats, fallback to curated defaults |
| `alphabetical` | Flat alphabetical list, no tiers |
| `curated` | Always use hardcoded default order (never personalize) |

---

## 6. Implementation Notes

### Integration Point: Clap Custom Help

wg uses `clap` for argument parsing. Custom help formatting requires:

```rust
use clap::{Command, CommandFactory};

fn print_custom_help(stats: &UsageStats, config: &Config) {
    let cmd = Cli::command();
    let subcommands = cmd.get_subcommands();

    let ordering = match config.help_ordering {
        Ordering::Alphabetical => alphabetical_order(subcommands),
        Ordering::Curated => curated_order(subcommands),
        Ordering::Usage => usage_order(subcommands, stats),
    };

    // Render with tiers...
}
```

Override the default `--help` handler in clap to call custom formatting:

```rust
let cli = Cli::command()
    .disable_help_flag(true)
    .arg(
        Arg::new("help")
            .long("help")
            .action(ArgAction::SetTrue)
    );

if cli.help {
    print_custom_help(&stats, &config);
    return Ok(());
}
```

### No-Args Behavior

When the user runs `wg` with no arguments, show the same help as `wg --help`. This is already the clap default behavior when no subcommand is provided.

### Performance

Reading stats for `--help` should be fast:
- Stats file is ~1KB (50 commands × 20 bytes)
- JSON parse: sub-millisecond
- No locking needed for read (tolerate slightly stale data)

Track the overhead and ensure total --help latency stays under 50ms.

---

## 7. Example Outputs

### With Usage Stats

```
wg - wg task management

Your most-used:
  list        List tasks with filters
  ready       Show tasks ready to start
  done        Mark task as complete

Also used:
  show        Display task details
  add         Create a new task
  log         Add progress entry
  claim       Claim a task for yourself

More commands (--help-all for full list):
  fail        Mark task as failed
  spawn       Start an agent
  status      Show project status
  ... and 40 more

Options:
  -d, --dir <PATH>  wg directory [default: .wg]
  -h, --help        Print help (--help-all for all commands)
  -V, --version     Print version
```

### Without Stats (Cold Start)

```
wg - wg task management

Commands:
  list        List tasks with filters
  ready       Show tasks ready to start
  status      Show project status
  show        Display task details
  add         Create a new task
  done        Mark task as complete
  claim       Claim a task for yourself
  fail        Mark task as failed
  log         Add progress entry
  ... and 40 more (--help-all)

Options:
  -d, --dir <PATH>  wg directory [default: .wg]
  -h, --help        Print help (--help-all for all commands)
  -V, --version     Print version
```

### Alphabetical Mode (Opt-Out)

```
wg - wg task management

Commands:
  add         Create a new task
  agents      List running agents
  analyze     Run dependency analysis
  artifact    Record output file
  claim       Claim a task for yourself
  config      Manage configuration
  context     Show task context
  ...

Options:
  -d, --dir <PATH>  wg directory [default: .wg]
  -h, --help        Print help
  -V, --version     Print version
```

---

## 8. Summary

| Aspect | Decision | Rationale |
|--------|----------|-----------|
| **Display** | Tiered grouping (frequent/occasional/rare) | Reduces cognitive load, highlights core commands |
| **Counts shown** | No | Adds noise; tiers communicate relative use |
| **Fallback** | Curated default order when <20 total invocations | Sensible UX before personalization kicks in |
| **Decay** | None (raw counts) | Simplicity; manual reset available |
| **Scope** | Per-repo only | Projects have different command profiles |
| **Opt-out** | Config (`help.ordering`) + flag (`--alphabetical`) | User choice without complexity |
| **Truncation** | Show ~15 commands, `--help-all` for full | Keeps default output scannable |

---

## 9. Open Questions

1. **Tier labels:** "Your most-used" vs "Frequent" vs "Common"? Need to test what reads naturally.
2. **Threshold tuning:** 10%/2% breakpoints are estimates. May need adjustment after real usage.
3. **Stats file in .gitignore?** Probably yes (personal usage shouldn't be shared), but some teams might want shared stats.
