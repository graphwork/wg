# Screencast Freshness: Auto-Regeneration Pipeline

*Architecture for keeping screencasts current as the TUI evolves.*

**Produced by:** design-screencast-freshness  
**Date:** 2026-03-23  
**Status:** Architecture complete, ready for implementation  
**Dependencies:** implement-tui-event (TUI event tracing), research-demo-medley (scenario catalog)

---

## Table of Contents

1. [Problem Statement](#problem-statement)
2. [Key Decision: Two-Track Rendering](#key-decision-two-track-rendering)
3. [Architecture Overview](#architecture-overview)
4. [Component Design](#component-design)
5. [CI Integration](#ci-integration)
6. [Change Detection](#change-detection)
7. [Edge Cases and Fallbacks](#edge-cases-and-fallbacks)
8. [File Layout](#file-layout)
9. [Implementation Effort](#implementation-effort)
10. [Assumptions and Constraints](#assumptions-and-constraints)

---

## Problem Statement

Screencasts are documentation. Stale screencasts actively mislead users — a screencast showing an old TUI layout is worse than no screencast at all.

The current pipeline (`record-auto.sh`, `record-pancakes-sim.sh`) produces screencasts manually. There is no mechanism to detect when a screencast becomes stale, and re-recording requires human intervention for every scenario.

**Goal:** Build a pipeline where screencast *sources* (event traces, scenario specs) are version-controlled, and `.cast` files are treated as **build artifacts** that can be regenerated automatically when TUI rendering changes.

---

## Key Decision: Two-Track Rendering

There are two fundamentally different kinds of screencasts, and they need different rendering strategies.

### Track A: Simulation-based (CLI-driven state progression)

**For:** Scenarios demonstrating graph state transitions, CLI commands, service lifecycle.  
**Source of truth:** Scenario spec files (`.toml`).  
**Mechanism:** Drive `wg` CLI commands via tmux `send-keys`, capture terminal output with `capture-tmux.py`.  
**Existing prototype:** `record-pancakes-sim.sh`.

Track A does not exercise TUI rendering internals — it captures the *external* terminal output of a real `wg tui` process driven by scripted inputs. When the TUI changes, re-running the same spec against the new binary produces updated output.

**Applies to scenarios:** #1 First Five Minutes, #3 Validation Gates, #4 Dependency Analysis, #5 Cycles, #10 Messaging (from demo-medley-catalog.md).

### Track B: Trace replay (headless TUI rendering)

**For:** Scenarios showcasing TUI-specific interactions (edge tracing, navigation, panel switching).  
**Source of truth:** Event trace files (`.jsonl` from `wg tui --trace`) + graph snapshots.  
**Mechanism:** Feed recorded events into a headless TUI (ratatui `TestBackend`), capture each rendered frame, emit `.cast` format.

Track B exercises the actual TUI rendering code path. When rendering logic in `src/tui/` changes, replaying the same event trace produces visually different output — which is exactly what we want to detect.

**Applies to scenarios:** #2 Chat Decomposition (refresh), #7 Edge Tracing, #9 Agency System.

### Decision rationale

| Factor | Track A (Simulation) | Track B (Trace Replay) |
|--------|---------------------|----------------------|
| Exercises TUI rendering code | No (captures external output) | **Yes** (renders internally) |
| Detects rendering regressions | Only visually (frame diff) | **Structurally** (same inputs → different outputs) |
| Requires tmux | Yes | No |
| Requires headless mode | No | **Yes** (new component) |
| Graph state management | Driven by CLI commands | **Snapshot required** |
| Existing infra | Prototype exists | **New build** |

Both tracks converge on the same output (`.cast` files) and share the same compression (`compress-cast.py`) and diff detection stages.

---

## Architecture Overview

```
Source of Truth                     Rendering                      Output
───────────────                     ─────────                      ──────

┌──────────────┐
│ .toml specs  │──── Track A ─────┐
│ (scenarios)  │   (tmux sim)     │
└──────────────┘                  │     ┌───────────┐     ┌──────────────┐
                                  ├────▶│ raw .cast │────▶│compress-cast │
┌──────────────┐                  │     └───────────┘     └──────┬───────┘
│ .jsonl traces│──── Track B ─────┘                              │
│ + .jsonl snap│   (headless TUI)                                ▼
└──────────────┘                                          ┌──────────────┐
                                                          │  final .cast │
                                                          └──────┬───────┘
                                                                 │
                                                                 ▼
                                                          ┌──────────────┐
                                                          │  diff detect │
                                                          └──────┬───────┘
                                                          ┌──────┴──────┐
                                                          ▼             ▼
                                                     No change     Changed
                                                     (skip)     (commit/flag)
```

### Lifecycle: record → store → re-render → detect → update

| Phase | Track A | Track B |
|-------|---------|---------|
| **Record** | Author writes `.toml` scenario spec | Run `wg tui --trace trace.jsonl` during a session; graph snapshot captured automatically |
| **Store** | `screencast/scenarios/*.toml` (version-controlled) | `screencast/traces/*.jsonl` + `screencast/snapshots/*.jsonl` (version-controlled) |
| **Re-render** | `screencast/record-sim.sh <spec.toml>` → raw `.cast` | `wg screencast replay <trace.jsonl>` → raw `.cast` |
| **Detect** | `screencast/diff-cast.py old.cast new.cast` | Same tool |
| **Update** | Auto-commit if within threshold, else flag for review | Same policy |

---

## Component Design

### 1. Scenario Spec Format (Track A)

Formalize the pattern from `record-pancakes-sim.sh` into a declarative TOML spec, as proposed in the demo-medley-catalog.

```toml
[meta]
name = "validation-gates"
description = "Shows --verify flag, pending-validation, and wg approve flow"
duration_target = 30       # seconds, compressed
terminal = { cols = 120, rows = 35 }
track = "simulation"       # "simulation" or "trace-replay"

[graph]
# Optional: pre-populate chat history
chat_history = "scenarios/validation-gates-chat.json"

[[graph.tasks]]
id = "implement-feature"
title = "Implement auth endpoint"
verify = "cargo test test_auth passes"

[[graph.tasks]]
id = "deploy-staging"
title = "Deploy to staging"
after = ["implement-feature"]

[[progression]]
action = "claim"
task = "implement-feature"
delay = 2.0

[[progression]]
action = "done"
task = "implement-feature"
delay = 4.0

[[progression]]
action = "approve"
task = "implement-feature"
delay = 3.0

[[keystrokes]]
time = 0.0
keys = "wg tui\n"

[[keystrokes]]
time = 15.0
keys = ["Down", "Down", "t"]
```

**Runner:** `screencast/record-sim.sh` reads the spec, sets up a temp project, drives tmux, and captures with `capture-tmux.py`. This generalizes the existing `record-pancakes-sim.sh`.

### 2. Graph Snapshot on Trace (Track B prerequisite)

When `wg tui --trace <path>` starts, automatically copy `.wg/graph.jsonl` to `<path>.snap.jsonl`. This captures the graph state at the moment the trace begins, ensuring replay starts from identical state.

**Implementation:** In `src/tui/viz_viewer/mod.rs`, after opening the EventTracer, copy the graph file. ~20 lines of code.

Additionally, write a metadata header as the first line of the trace:

```json
{"_meta": true, "version": 1, "terminal": {"cols": 120, "rows": 35}, "graph_snapshot": "trace.jsonl.snap.jsonl", "wg_version": "0.1.0", "timestamp": "2026-03-23T15:30:00Z"}
```

This pins the trace to a specific TUI version, terminal size, and graph state.

### 3. TracePlayer (deserialize + reconstruct events)

New module: `src/tui/viz_viewer/replay.rs`

Responsibilities:
- Deserialize `TraceEntry` objects from JSONL
- Reconstruct `crossterm::Event` from `TracedEvent` (reverse of `EventTracer::record`)
- Provide an iterator of `(timestamp, Event, StateContext)` tuples
- Validate state context at each step (optional — log warnings when replay state diverges from recorded state)

```rust
pub struct TracePlayer {
    entries: Vec<TraceEntry>,
    cursor: usize,
}

impl TracePlayer {
    pub fn from_file(path: &Path) -> Result<Self>;
    pub fn next_event(&mut self) -> Option<(f64, Event, StateContext)>;
    pub fn metadata(&self) -> &TraceMeta;
}
```

Key design choice: The player reconstructs `crossterm::Event` values that are fed into the existing `dispatch_event` function. This means replay exercises the *exact same event handling code* as live usage — no separate replay code path to maintain.

### 4. Headless Replay Mode (`wg screencast replay`)

New subcommand: `wg screencast replay <trace.jsonl> [--output recording.cast] [--graph snapshot.jsonl]`

Implementation:
1. Load graph snapshot into a new `.wg/` in a temp directory
2. Create a `VizApp` from the loaded graph state
3. Create a ratatui `Terminal<TestBackend>` with dimensions from trace metadata
4. Create an asciinema `.cast` writer (header + frame stream)
5. For each event from `TracePlayer`:
   a. Construct the `crossterm::Event`
   b. Call `dispatch_event(app, event)`
   c. Render to `TestBackend`
   d. Diff the buffer against the previous frame
   e. If changed, convert to ANSI text and write a `.cast` frame at the recorded timestamp
6. Flush and close the `.cast` file

**Frame conversion:** ratatui's `TestBackend` provides a `Buffer` with cells containing characters, styles, and colors. Convert each cell row to an ANSI-escaped string using ratatui's built-in serialization. This produces terminal output equivalent to what a real terminal would display.

**Timing:** Use the recorded timestamps from the trace to set frame times. This preserves the original interaction timing, which `compress-cast.py` can then optimize.

### 5. Diff Detection (`screencast/diff-cast.py`)

New script that compares two `.cast` files and reports whether they are "significantly different."

**Algorithm:**
1. Parse both `.cast` files into frame sequences
2. Strip ANSI escape codes from each frame (text-only comparison)
3. Align frames by timestamp (nearest-neighbor within 0.5s tolerance)
4. For each aligned pair, compute line-level text diff
5. Score: percentage of frames with >5% changed lines

**Thresholds:**
- **<5% frames changed** → Cosmetic (timestamps, counters). Auto-accept.
- **5–30% frames changed** → Layout adjustment. Auto-commit with a diff summary in the commit message.
- **>30% frames changed** → Significant change. Flag for human review (open a GitHub issue or PR comment).

**Additional checks:**
- Frame count difference >20% → structural change → flag for review
- Missing frames at the end → possible replay truncation → flag

Output format:
```
$ screencast/diff-cast.py old.cast new.cast
Frames: 142 vs 148 (+4.2%)
Changed frames: 12/142 (8.4%) — layout adjustment
Biggest change at t=12.3s: 18 lines differ (right panel resized)
Verdict: AUTO-COMMIT
```

### 6. The `wg screencast` Subcommand

Collect all screencast operations under a single subcommand:

```
wg screencast record <scenario.toml>    # Track A: simulation recording
wg screencast replay <trace.jsonl>      # Track B: headless replay
wg screencast diff <old.cast> <new.cast> # Compare two recordings
wg screencast render-all                 # Re-render all scenarios
wg screencast status                     # Show freshness status per scenario
```

`render-all` iterates over registered scenarios, re-renders each, and reports diff results. This is the CI entry point.

`status` shows each scenario's last render date, current wg version, and whether a re-render would produce different output (dry-run diff).

---

## CI Integration

### GitHub Actions Workflow: `.github/workflows/screencast-freshness.yml`

```yaml
name: Screencast Freshness
on:
  pull_request:
    paths:
      - 'src/tui/**'
      - 'src/commands/viz/**'
      - 'screencast/**'
  push:
    branches: [main]
    paths:
      - 'src/tui/**'
      - 'src/commands/viz/**'
      - 'screencast/**'
  workflow_dispatch: {}

jobs:
  check-freshness:
    runs-on: ubuntu-latest
    steps:
      - uses: actions/checkout@v4

      - name: Install Rust toolchain
        uses: dtolnay/rust-toolchain@stable

      - name: Build wg
        run: cargo install --path .

      - name: Install tmux
        run: sudo apt-get install -y tmux

      - name: Re-render all screencasts
        run: wg screencast render-all --output-dir /tmp/screencast-check

      - name: Compare with committed versions
        id: diff
        run: |
          CHANGED=0
          for cast in screencast/recordings/*.cast; do
            name=$(basename "$cast")
            new="/tmp/screencast-check/$name"
            if [ -f "$new" ]; then
              result=$(screencast/diff-cast.py "$cast" "$new" --machine-readable)
              verdict=$(echo "$result" | jq -r '.verdict')
              if [ "$verdict" != "IDENTICAL" ]; then
                echo "::warning::$name: $verdict"
                CHANGED=1
              fi
            fi
          done
          echo "changed=$CHANGED" >> "$GITHUB_OUTPUT"

      - name: Post PR comment (if changed)
        if: github.event_name == 'pull_request' && steps.diff.outputs.changed == '1'
        uses: actions/github-script@v7
        with:
          script: |
            github.rest.issues.createComment({
              owner: context.repo.owner,
              repo: context.repo.repo,
              issue_number: context.issue.number,
              body: '⚠️ **Screencast freshness:** This PR changes TUI rendering. Some screencasts may need updating. Run `wg screencast render-all` locally to review.'
            })
```

### Trigger Matrix

| Code change | Screencasts affected | CI action |
|-------------|---------------------|-----------|
| `src/tui/viz_viewer/render.rs` | All TUI screencasts (Track A + B) | Re-render all, flag diffs |
| `src/tui/viz_viewer/state.rs` | All TUI screencasts | Re-render all, flag diffs |
| `src/tui/viz_viewer/event.rs` | Track B screencasts (event handling changed) | Re-render trace replays |
| `src/commands/viz/mod.rs` | `wg viz` screencasts only | Re-render affected specs |
| `screencast/compress-cast.py` | All (compression changed) | Re-compress all |
| `screencast/scenarios/*.toml` | Specific scenario | Re-render that scenario |
| `screencast/traces/*.jsonl` | Specific trace | Re-render that trace |
| Release tag | All | Full re-render + commit |

### Auto-commit on main

When a push to `main` triggers a re-render and the diff is within auto-commit threshold (<30% frames changed):

```yaml
      - name: Auto-commit updated screencasts
        if: github.ref == 'refs/heads/main' && steps.diff.outputs.changed == '1'
        run: |
          git config user.name "wg-screencast-bot"
          git config user.email "bot@wg.dev"
          git add screencast/recordings/*.cast
          git commit -m "chore: auto-regenerate screencasts for $(git rev-parse --short HEAD)"
          git push
```

For >30% changes, open an issue instead of auto-committing.

---

## Change Detection

### Two-tier detection model

**Tier 1 — Text diff (CI gate, always runs):**
- Strip ANSI codes from frames
- Compare text content line-by-line
- Fast, deterministic, catches layout/content changes
- Threshold: frames with >5% line-level differences are "changed"

**Tier 2 — Raw frame hash (optional, for style changes):**
- Hash each frame *including* ANSI escape codes
- Detects color/style changes that text diff misses
- More sensitive — useful for catching theme regressions
- Run as a separate CI check (non-blocking)

### What counts as "cosmetic" vs "significant"

| Change type | Classification | Example |
|-------------|---------------|---------|
| Timestamp in agent log | Cosmetic | `15:30:01` → `15:30:02` |
| Task status color change | Style (Tier 2 only) | Yellow → amber for `in-progress` |
| Panel width change | Significant | Right panel 40→50 cols |
| New panel/section | Significant | Added "Firehose" tab |
| Task order change | Significant | Different sort in task list |
| Missing content | Critical | Panel that existed is gone |

---

## Edge Cases and Fallbacks

### 1. Broken trace replay (removed feature)

**Scenario:** A trace records pressing `x` to toggle a feature. That key binding is removed in a later version.

**Detection:** The `TracePlayer` records the expected `StateContext` at each event. After dispatching the event, compare the actual VizApp state against the expected state. Divergence beyond a threshold (e.g., different focused panel, different selected task) triggers a warning.

**Fallback:** Log the divergence point. Mark the screencast as **stale — manual re-record required**. Open a GitHub issue:
```
Screencast 'edge-tracing' replay diverged at t=5.2s:
  Expected: focused_panel=graph, right_panel_tab=detail
  Actual:   focused_panel=graph, right_panel_tab=log
  
  The trace may reference a removed feature or changed key binding.
  Manual re-recording is needed.
```

### 2. Layout changes (significant visual difference)

**Scenario:** The TUI layout changes (new panel, different proportions). The trace replays successfully (events still dispatch) but the visual output is very different.

**Detection:** Frame diff catches this (>30% frames changed).

**Fallback:** Flag for review. The trace is still valid (events replay) but the screencast may no longer tell the intended story. A human should review whether the new layout still demonstrates the feature, or whether the scenario spec needs updating.

### 3. Terminal size mismatch

**Scenario:** A trace was recorded at 120×35 but the replay environment is 80×24.

**Detection:** Compare trace metadata terminal size against the replay `TestBackend` size.

**Fallback:** Use the trace metadata's terminal size for the `TestBackend`. This is controlled by the replay code, not the host terminal, so it should always match. If the metadata is missing (old traces without the header), default to 120×35 with a warning.

### 4. Graph state drift

**Scenario:** The graph snapshot references task fields or statuses that the current `wg` version doesn't understand (schema evolution).

**Detection:** `VizApp` initialization will log parsing warnings or fail to load tasks.

**Fallback:** If the graph can't load, the trace can't replay. Mark as stale. Graph snapshots should use the same JSONL format as `graph.jsonl` — the same migration logic that handles on-disk graphs handles snapshots.

### 5. Flaky frame timing

**Scenario:** The headless replay produces frames at slightly different timestamps than the original (due to rendering speed differences), causing false-positive diffs.

**Mitigation:** The diff tool aligns frames by nearest timestamp within a tolerance window (0.5s). Frame *content* is compared, not frame *timing*. `compress-cast.py` normalizes timing anyway.

### 6. Simulation spec becomes invalid

**Scenario:** A Track A `.toml` spec references a `wg` subcommand or flag that was renamed or removed.

**Detection:** `record-sim.sh` exits non-zero because the `wg` command fails.

**Fallback:** CI reports the failure. The spec needs updating — this is analogous to a broken test. The spec is code and should be maintained alongside the features it demonstrates.

---

## File Layout

```
screencast/
├── README.md                         # Updated with freshness pipeline docs
├── capture-tmux.py                   # Existing: tmux → .cast converter
├── compress-cast.py                  # Existing: time compression
├── diff-cast.py                      # NEW: frame-level diff detection
├── record-sim.sh                     # NEW: generic simulation runner (replaces per-scenario scripts)
├── record-auto.sh                    # Existing: live recording (kept for non-simulatable scenarios)
├── scenarios/                        # NEW: Track A source of truth
│   ├── first-five-minutes.toml
│   ├── validation-gates.toml
│   ├── validation-gates-chat.json    # Pre-populated chat history
│   ├── cycles.toml
│   ├── dependency-analysis.toml
│   └── ...
├── traces/                           # NEW: Track B source of truth
│   ├── edge-tracing.jsonl            # Event trace from wg tui --trace
│   ├── edge-tracing.jsonl.snap.jsonl # Graph snapshot at trace time
│   └── ...
├── recordings/                       # Build artifacts (may be .gitignored on main, committed for releases)
│   ├── first-five-minutes-raw.cast
│   ├── first-five-minutes.cast       # Compressed final
│   └── ...
└── legacy/                           # Existing per-scenario scripts (deprecated, kept for reference)
    ├── record-pancakes-sim.sh
    ├── record-heist-auto.sh
    └── setup-demo.sh
```

The `scenarios/` and `traces/` directories are the **source of truth**. The `recordings/` directory contains build artifacts.

---

## Implementation Effort

| # | Component | Track | Effort | Description |
|---|-----------|-------|--------|-------------|
| 1 | Scenario spec parser + generic `record-sim.sh` | A | **2–3 days** | TOML parser (Python or shell), generalize `record-pancakes-sim.sh` into a spec-driven runner. Largest Track A component. |
| 2 | Graph snapshot on `--trace` | B | **0.5 day** | Copy `graph.jsonl` + write trace metadata header when `EventTracer` opens. ~30 lines in `mod.rs` + `trace.rs`. |
| 3 | `TraceEntry` deserialization | B | **0.5 day** | Add `Deserialize` to `TraceEntry`, `TracedEvent`, `StateContext`. Add `Event` reconstruction from `TracedEvent`. |
| 4 | `TracePlayer` module (`replay.rs`) | B | **1–2 days** | File reader, event iterator, state validation logic. |
| 5 | Headless replay mode + `.cast` writer | B | **2–3 days** | `TestBackend` rendering, buffer-to-ANSI conversion, `.cast` frame writer. The core of Track B. |
| 6 | `diff-cast.py` | Shared | **1–2 days** | ANSI stripping, frame alignment, text diff, scoring, threshold logic, machine-readable output for CI. |
| 7 | `wg screencast` subcommand | Shared | **1 day** | CLI wiring: `record`, `replay`, `diff`, `render-all`, `status`. Dispatches to components above. |
| 8 | CI workflow | Shared | **1 day** | GitHub Actions YAML, trigger configuration, PR comment logic, auto-commit on main. |
| 9 | First scenario specs (Batch 1) | A | **1–2 days** | Port `record-pancakes-sim.sh` to spec format. Write specs for First Five Minutes, Validation Gates, Cycles, Dependency Analysis. |
| 10 | First trace recordings (Batch 1) | B | **0.5 day** | Record edge-tracing trace with `wg tui --trace`. |
| 11 | Documentation + migration | Shared | **0.5 day** | Update `screencast/README.md`, deprecate legacy scripts. |

**Total estimated effort: 11–16 days**

### Recommended implementation order

```
Phase 1 (Track A — quick wins):       ~4 days
  1 → 9 → 6 → 8
  Delivers: simulatable scenarios + CI freshness check

Phase 2 (Track B — headless replay):  ~5 days
  2 → 3 → 4 → 5 → 10
  Delivers: trace replay producing .cast files

Phase 3 (Polish):                      ~2 days
  7 → 11
  Delivers: unified CLI, documentation
```

Phase 1 is independently valuable — it automates the existing simulation approach. Phase 2 adds the more powerful trace replay capability. Phase 3 unifies the interface.

---

## Assumptions and Constraints

### Assumptions

1. **ratatui `TestBackend` produces representative output.** The headless buffer should match real terminal rendering. Known limitation: `TestBackend` doesn't support true terminal features (256-color palette mapping, Unicode width edge cases). For screencast purposes, this is acceptable — the goal is to detect *changes*, not pixel-perfect reproduction.

2. **Event traces are forward-compatible within minor versions.** A trace recorded with wg v0.5.0 should replay on v0.5.x. Major version bumps may break traces, requiring re-recording.

3. **tmux is available in CI.** Track A simulation requires tmux. This is standard on GitHub Actions Ubuntu runners.

4. **Graph JSONL format is stable.** Snapshots use the same format as `graph.jsonl`. If the format changes, existing snapshots need migration (same as live graphs).

5. **Compressed `.cast` files are the deployment artifact.** Raw recordings are intermediate. The compressed version is what gets committed and deployed to the website.

### Constraints

1. **No LLM calls in CI.** All CI-rendered screencasts must be fully deterministic. Scenarios requiring live LLM responses (Chat Decomposition with real coordinator) are recorded manually and kept as static assets, not auto-regenerated.

2. **Trace replay is TUI-only.** Track B only works for `wg tui` sessions. CLI screencasts (Track A) use simulation. There is no "headless CLI replay" — CLI output is captured externally via tmux.

3. **No cross-platform rendering.** Headless replay uses a single `TestBackend` implementation. Platform-specific rendering differences (macOS Terminal.app vs. Linux GNOME Terminal) are not captured. This is acceptable because asciinema playback already normalizes rendering.

4. **Maximum trace size.** A 60-second session at 10 events/second produces ~600 trace entries (~100KB). A 5-minute session produces ~3000 entries (~500KB). Traces should be kept under 5 minutes; longer sessions should be split.

5. **Frame diff is text-based, not visual.** We compare stripped text, not rendered pixels. Subtle visual regressions (off-by-one column, wrong bold attribute) may not be caught by Tier 1. Tier 2 (raw hash) catches most of these, but it's optional.

---

## Version Pinning

Each trace and scenario spec is tied to a wg version via metadata:

- **Traces:** `_meta.wg_version` field in the trace header
- **Scenario specs:** `[meta] wg_version = "0.5.0"` field

The `wg screencast status` command compares pinned versions against the current binary and flags stale sources:

```
$ wg screencast status
Scenario                   Track  Pinned    Current   Status
edge-tracing               B      v0.5.0    v0.5.2    OK (minor)
validation-gates           A      v0.4.0    v0.5.2    STALE (major bump)
first-five-minutes         A      v0.5.1    v0.5.2    OK (patch)
```

Minor version bumps are expected to be compatible. Major bumps flag sources as potentially stale, even if replay succeeds — the scenario may not demonstrate current behavior.

---

*End of architecture document. This is an artifact of task design-screencast-freshness.*
