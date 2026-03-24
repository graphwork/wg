#!/usr/bin/env python3
"""Record a focused screencast: graph navigation and detail views.

Demonstrates:
  - Task list view with arrow Up/Down navigation
  - Tab switching between left/right panel focus
  - All 4 detail panes: 1=Detail, 2=Log, 3=Messages, 4=Agency
  - Key feedback overlay (--show-keys)

Target duration: 10-20 seconds.
Output: screencast/recordings/screencast-graph-nav.cast
"""

import json
import os
import random
import subprocess
import sys
import time

random.seed(42)

# Import the recording harness
sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
import importlib
record_harness = importlib.import_module("record-harness")
RecordingHarness = record_harness.RecordingHarness
_verify_cast = record_harness._verify_cast

SCRIPT_DIR = os.path.dirname(os.path.abspath(__file__))
RECORDINGS_DIR = os.path.join(SCRIPT_DIR, "recordings")
DEMO_DIR = f"/tmp/wg-graph-nav-{os.getpid()}"

# Tasks for a realistic graph — mix of statuses and dependencies
TASKS = [
    ("Scrape headlines", "scrape-headlines", None,
     "Fetch top news headlines from RSS feeds", "done"),
    ("Analyze mood", "analyze-mood", "scrape-headlines",
     "Sentiment analysis on each headline", "in-progress"),
    ("Count syllables", "count-syllables", None,
     "Build syllable-counting engine", "done"),
    ("Build pun db", "build-pun-db", None,
     "Collect wordplay database for haiku generation", "in-progress"),
    ("Wire haiku engine", "wire-haiku-engine", "scrape-headlines,count-syllables",
     "Core haiku generator combining headlines + syllable rules", "open"),
    ("Draft haikus", "draft-haikus", "wire-haiku-engine,analyze-mood",
     "Generate haiku for each headline using mood + engine", "open"),
    ("Review quality", "review-quality", "draft-haikus",
     "Quality gate on generated haiku — syllable counts + topic relevance", "open"),
    ("Publish API", "publish-api", "review-quality,build-pun-db",
     "REST API serving approved haiku with pun database enrichment", "open"),
]

FAKE_LOGS = {
    "scrape-headlines": [
        "Fetching RSS feeds from 5 sources...",
        "Parsed 47 headlines, deduped to 32 unique",
        "Validated: all headlines have title + url + timestamp",
    ],
    "analyze-mood": [
        "Loading sentiment model (distilbert-base-uncased-finetuned-sst-2)",
        "Processing headline batch 1/4 (8 headlines)...",
        "Batch 1 complete: 5 positive, 2 neutral, 1 negative",
    ],
    "count-syllables": [
        "Built CMU pronunciation dictionary lookup",
        "Added fallback heuristic for unknown words",
        "Validated: 98.3% accuracy on test corpus (1200 words)",
    ],
    "build-pun-db": [
        "Scraping pun databases (3 sources)...",
        "Collected 2,847 puns, indexing by keyword...",
    ],
}

FAKE_MESSAGES = {
    "wire-haiku-engine": [
        ("user", "Make sure the engine handles compound words correctly"),
        ("assistant", "Will add compound-word splitting before syllable count"),
    ],
}

_start_time = None


def log(msg):
    elapsed = time.monotonic() - _start_time if _start_time else 0
    print(f"[{elapsed:7.1f}s] {msg}", file=sys.stderr)


def wg(*args):
    try:
        return subprocess.run(
            ["wg"] + list(args),
            capture_output=True, text=True,
            cwd=DEMO_DIR, timeout=30,
        )
    except subprocess.TimeoutExpired:
        return None


def setup_demo():
    """Create a demo project with tasks, logs, and messages."""
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

    # Add tasks
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

    # Add log entries
    for tid, entries in FAKE_LOGS.items():
        for entry in entries:
            wg("log", tid, entry)
            time.sleep(0.05)

    # Add messages
    for tid, msgs in FAKE_MESSAGES.items():
        for role, text in msgs:
            wg("msg", "send", tid, text)
            time.sleep(0.05)

    # Inject chat history
    chat_path = os.path.join(DEMO_DIR, ".workgraph", "chat-history.json")
    os.makedirs(os.path.dirname(chat_path), exist_ok=True)
    with open(chat_path, "w") as f:
        json.dump([
            {
                "role": "user",
                "text": "Build a haiku news pipeline — scrape headlines, generate haiku for each, and publish an API",
                "timestamp": "2026-03-24T15:00:01+00:00",
                "edited": False,
            },
            {
                "role": "assistant",
                "text": (
                    "I'll build the haiku news pipeline:\n\n"
                    "1. **scrape-headlines** — fetch RSS headlines\n"
                    "2. **analyze-mood** — sentiment (after headlines)\n"
                    "3. **count-syllables** — syllable engine (parallel)\n"
                    "4. **build-pun-db** — wordplay database (parallel)\n"
                    "5. **wire-haiku-engine** — core generator (after 1,3)\n"
                    "6. **draft-haikus** — generate haiku (after 5,2)\n"
                    "7. **review-quality** — quality gate (after 6)\n"
                    "8. **publish-api** — REST API (after 7,4)\n\n"
                    "Creating tasks now..."
                ),
                "timestamp": "2026-03-24T15:00:10+00:00",
                "edited": False,
            },
        ], f, indent=2)

    log(f"Demo project ready at {DEMO_DIR} with {len(TASKS)} tasks")


def make_shell_cmd():
    return (
        f"cd {DEMO_DIR} && "
        f"export PS1='\\[\\033[1;32m\\]$ \\[\\033[0m\\]' && "
        f"exec bash --norc --noprofile"
    )


def record():
    """Record the focused graph navigation + detail views screencast."""
    global _start_time
    _start_time = time.monotonic()

    cast_file = os.path.join(RECORDINGS_DIR, "screencast-graph-nav.cast")
    log("=== Screencast: Graph Navigation & Detail Views ===")

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
        h.type_naturally("wg tui --show-keys", wpm=60)
        h.send_keys("Enter")
        found = h.wait_for("Chat", timeout=15, interval=0.5)
        if found:
            log("TUI rendered")
        else:
            log("WARNING: TUI not detected")
        h.sleep(0.8)
        h.flush_frame()

        # ── Phase 1: Arrow navigation through task list ──
        log("Phase 1: Arrow navigation down")
        for i in range(4):
            h.send_keys("Down")
            h.sleep(0.5)
            h.flush_frame()

        # Linger on an interesting task (wire-haiku-engine)
        h.sleep(0.8)

        # ── Phase 2: Cycle through detail panes 1-4 ──
        log("Phase 2: Detail pane 1 (Detail)")
        h.send_keys("1")
        h.sleep(1.2)
        h.flush_frame()

        log("Phase 2: Detail pane 2 (Log)")
        h.send_keys("2")
        h.sleep(1.2)
        h.flush_frame()

        log("Phase 2: Detail pane 3 (Messages)")
        h.send_keys("3")
        h.sleep(1.2)
        h.flush_frame()

        log("Phase 2: Detail pane 4 (Agency)")
        h.send_keys("4")
        h.sleep(1.2)
        h.flush_frame()

        # ── Phase 3: Tab to switch panel focus ──
        log("Phase 3: Tab to right panel")
        h.send_keys("Tab")
        h.sleep(1.0)
        h.flush_frame()

        # Navigate in the right panel
        h.send_keys("Up")
        h.sleep(0.4)
        h.send_keys("Up")
        h.sleep(0.4)
        h.flush_frame()

        # Tab back to left panel (task list)
        log("Phase 3: Tab back to task list")
        h.send_keys("Tab")
        h.sleep(0.8)
        h.flush_frame()

        # ── Phase 4: Navigate up to a different task ──
        log("Phase 4: Arrow up")
        for i in range(3):
            h.send_keys("Up")
            h.sleep(0.5)
            h.flush_frame()

        # Show Detail pane for the new task
        h.send_keys("1")
        h.sleep(1.0)
        h.flush_frame()

        # Exit
        h.send_keys("q")
        h.sleep(0.5)

        duration = h.duration
        log(f"Recording complete: {duration:.1f}s, {h.frame_count} frames")

    # Verify
    log("Verifying cast file...")
    _verify_cast(cast_file)

    return cast_file


if __name__ == "__main__":
    result = record()
    if result:
        print(f"\nOutput: {result}", file=sys.stderr)
        sys.exit(0)
    else:
        sys.exit(1)
