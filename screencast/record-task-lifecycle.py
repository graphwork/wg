#!/usr/bin/env python3
"""Record a 10-15s screencast showing task lifecycle in the TUI.

Demonstrates:
  - Multiple task states (done, in-progress, open, failed)
  - Dependency edges visible in graph view
  - Detail pane switching with 1-4 keys
  - Key feedback overlay (--show-keys)

Output: screencast/recordings/tui-task-lifecycle-raw.cast
"""

import json
import os
import random
import subprocess
import sys
import time

random.seed(99)

# Import the recording harness
sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
import importlib
record_harness = importlib.import_module("record-harness")
RecordingHarness = record_harness.RecordingHarness
_verify_cast = record_harness._verify_cast

SCRIPT_DIR = os.path.dirname(os.path.abspath(__file__))
RECORDINGS_DIR = os.path.join(SCRIPT_DIR, "recordings")
DEMO_DIR = f"/tmp/wg-task-lifecycle-{os.getpid()}"

# Tasks showing a clear lifecycle with multiple states and dependency edges
TASKS = [
    # (title, id, after, description, target_status)
    ("Fetch data", "fetch-data", None,
     "Download raw dataset from upstream API", "done"),
    ("Validate schema", "validate-schema", "fetch-data",
     "Check dataset conforms to expected schema", "done"),
    ("Transform records", "transform-records", "validate-schema",
     "Normalize and clean the dataset", "in-progress"),
    ("Build index", "build-index", "validate-schema",
     "Create searchable index from validated data", "failed"),
    ("Run analysis", "run-analysis", "transform-records",
     "Statistical analysis on transformed data", "open"),
    ("Generate report", "generate-report", "run-analysis,build-index",
     "Produce final report combining analysis + index", "open"),
]

FAKE_LOGS = {
    "fetch-data": [
        "Connecting to upstream API...",
        "Downloaded 2,340 records (1.2 MB)",
        "Validated: all records have required fields",
    ],
    "validate-schema": [
        "Loading JSON schema v2.1...",
        "Validated 2,340/2,340 records — 0 errors",
    ],
    "transform-records": [
        "Normalizing date formats...",
        "Cleaning null fields (47 patched)",
        "Processing batch 2/3...",
    ],
    "build-index": [
        "Building trie from 2,340 records...",
        "ERROR: out of memory at record 1,892",
    ],
}

_start_time = None


def log(msg):
    elapsed = time.monotonic() - _start_time if _start_time else 0
    print(f"[{elapsed:7.1f}s] {msg}", file=sys.stderr)


def wg(*args):
    """Run wg command in the demo directory."""
    try:
        return subprocess.run(
            ["wg"] + list(args),
            capture_output=True, text=True,
            cwd=DEMO_DIR, timeout=30,
        )
    except subprocess.TimeoutExpired:
        return None


def setup_demo():
    """Create a demo project with tasks in various lifecycle states."""
    if os.path.exists(DEMO_DIR):
        subprocess.run(["rm", "-rf", DEMO_DIR])
    os.makedirs(DEMO_DIR)

    subprocess.run(["git", "init", "-q"], cwd=DEMO_DIR, check=True)
    subprocess.run(
        ["git", "commit", "--allow-empty", "-m", "init", "-q"],
        cwd=DEMO_DIR, check=True,
    )

    wg("init")

    # Hide system tasks for clean display
    config_path = os.path.join(DEMO_DIR, ".workgraph", "config.toml")
    with open(config_path) as f:
        config = f.read()
    if "show_system_tasks" in config:
        config = config.replace("show_system_tasks = true", "show_system_tasks = false")
    with open(config_path, "w") as f:
        f.write(config)

    # Add tasks with dependencies
    for title, tid, after, desc, _status in TASKS:
        cmd = ["add", title, "--id", tid, "-d", desc]
        if after:
            cmd.extend(["--after", after])
        wg(*cmd)
        time.sleep(0.15)

    # Set task statuses
    for _title, tid, _after, _desc, status in TASKS:
        if status == "done":
            wg("claim", tid)
            time.sleep(0.1)
            wg("done", tid)
            time.sleep(0.1)
        elif status == "in-progress":
            wg("claim", tid)
            time.sleep(0.1)
        elif status == "failed":
            wg("claim", tid)
            time.sleep(0.1)
            wg("fail", tid, "--reason", "Out of memory at record 1,892")
            time.sleep(0.1)

    # Add log entries
    for tid, entries in FAKE_LOGS.items():
        for entry in entries:
            wg("log", tid, entry)
            time.sleep(0.1)

    log(f"Demo project at {DEMO_DIR} with {len(TASKS)} tasks")


def make_shell_cmd():
    return (
        f"cd {DEMO_DIR} && "
        f"export PS1='\\[\\033[1;32m\\]$ \\[\\033[0m\\]' && "
        f"exec bash --norc --noprofile"
    )


def record_task_lifecycle():
    """Record the task lifecycle screencast."""
    global _start_time
    _start_time = time.monotonic()

    cast_file = os.path.join(RECORDINGS_DIR, "tui-task-lifecycle-raw.cast")
    log("=== Recording: Task Lifecycle ===")

    setup_demo()

    with RecordingHarness(
        cast_file=cast_file,
        cwd=DEMO_DIR,
        shell_command=make_shell_cmd(),
        idle_time_limit=2.0,
    ) as h:
        h.wait_for("$", timeout=5)
        h.sleep(0.3)

        # Launch TUI with key feedback overlay
        h.type_naturally("wg tui --show-keys", wpm=65)
        h.send_keys("Enter")
        found = h.wait_for("Chat", timeout=15, interval=0.5)
        if found:
            log("TUI rendered")
        else:
            log("WARNING: TUI render not detected")
        h.sleep(1.2)
        h.flush_frame()

        # --- Show graph with dependency edges (starts on graph view) ---
        # The graph view shows all tasks with edges between them.
        # Let the viewer see the full graph first.
        log("Showing graph overview with dependency edges")
        h.sleep(1.0)
        h.flush_frame()

        # Navigate down through tasks to highlight different states
        # Task order: fetch-data(done), validate-schema(done),
        #   transform-records(in-progress), build-index(failed),
        #   run-analysis(open), generate-report(open)
        log("Navigating through task states")
        h.send_keys("Down")   # -> validate-schema (done)
        h.sleep(0.6)
        h.flush_frame()

        h.send_keys("Down")   # -> transform-records (in-progress)
        h.sleep(0.6)
        h.flush_frame()

        h.send_keys("Down")   # -> build-index (failed)
        h.sleep(0.6)
        h.flush_frame()

        # Pause on the failed task — distinct state
        h.sleep(0.8)

        # --- Switch detail panes with 1-4 keys ---
        # Show Detail pane (1)
        log("Detail pane (1)")
        h.send_keys("1")
        h.sleep(1.2)
        h.flush_frame()

        # Show Log pane (2) — has error log
        log("Log pane (2)")
        h.send_keys("2")
        h.sleep(1.2)
        h.flush_frame()

        # Navigate to in-progress task to show different detail
        h.send_keys("Up")    # -> transform-records (in-progress)
        h.sleep(0.6)
        h.flush_frame()

        # Show Context pane (3)
        log("Context pane (3)")
        h.send_keys("3")
        h.sleep(1.0)
        h.flush_frame()

        # Show Agency pane (4)
        log("Agency pane (4)")
        h.send_keys("4")
        h.sleep(1.0)
        h.flush_frame()

        # Navigate down to open task
        h.send_keys("Down")   # -> build-index (failed)
        h.sleep(0.4)
        h.send_keys("Down")   # -> run-analysis (open)
        h.sleep(0.6)
        h.flush_frame()

        # Back to Detail (1) to show open task detail
        log("Detail pane (1) on open task")
        h.send_keys("1")
        h.sleep(1.0)
        h.flush_frame()

        # Exit
        h.send_keys("q")
        h.sleep(0.5)

        duration = h.duration
        log(f"Task lifecycle: {duration:.1f}s, {h.frame_count} frames")

    # Verify
    ok = _verify_cast(cast_file)
    log(f"Cast file: {cast_file}")
    return cast_file if ok else None


if __name__ == "__main__":
    result = record_task_lifecycle()
    sys.exit(0 if result else 1)
