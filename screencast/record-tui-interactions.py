#!/usr/bin/env python3
"""Record 4 focused TUI interaction examples with keystroke HUD overlay.

Each recording is 5-15 seconds, demonstrating one interaction pattern:
  1. Graph Navigation: Up/Down arrow scanning across tasks
  2. Inspector: Opening/closing inspector, exploring task details
  3. Agent Monitoring: Watching live agent activity via Firehose
  4. Key Workflows: Common keystroke combos (chat, navigate, inspect)

All recordings use --show-keys to display the keystroke HUD overlay,
so viewers can see exactly which keys are pressed.

Output: screencast/recordings/tui-{name}-raw.cast (4 files)
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
DEMO_DIR = f"/tmp/wg-tui-interactions-{os.getpid()}"

# Task definitions for a realistic-looking graph
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

# Fake log entries for tasks
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
    """Create a demo project with pre-populated tasks."""
    if os.path.exists(DEMO_DIR):
        subprocess.run(["rm", "-rf", DEMO_DIR])
    os.makedirs(DEMO_DIR)

    subprocess.run(["git", "init", "-q"], cwd=DEMO_DIR, check=True)
    subprocess.run(
        ["git", "commit", "--allow-empty", "-m", "init", "-q"],
        cwd=DEMO_DIR, check=True,
    )

    wg("init")

    # Configure for clean display
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
        time.sleep(0.2)

    # Set task statuses (claim and complete as needed)
    for _title, tid, _after, _desc, status in TASKS:
        if status == "done":
            wg("claim", tid)
            time.sleep(0.1)
            wg("done", tid)
            time.sleep(0.1)
        elif status == "in-progress":
            wg("claim", tid)
            time.sleep(0.1)

    # Add fake log entries
    for tid, entries in FAKE_LOGS.items():
        for entry in entries:
            wg("log", tid, entry)
            time.sleep(0.1)

    # Inject chat history for realism
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

    log(f"Demo project at {DEMO_DIR} with {len(TASKS)} tasks")


def make_shell_cmd():
    """Shell command for tmux that sets up the demo directory."""
    return (
        f"cd {DEMO_DIR} && "
        f"export PS1='\\[\\033[1;32m\\]$ \\[\\033[0m\\]' && "
        f"exec bash --norc --noprofile"
    )


def launch_tui(h):
    """Type 'wg tui --show-keys' and wait for it to render."""
    h.type_naturally("wg tui --show-keys", wpm=60)
    h.send_keys("Enter")
    found = h.wait_for("Chat", timeout=15, interval=0.5)
    if found:
        log("TUI rendered with --show-keys")
    else:
        log("WARNING: TUI render not detected")
    h.sleep(1)
    return found


def exit_tui(h):
    """Exit TUI cleanly."""
    h.send_keys("q")
    h.sleep(1)


# ── Recording 1: Graph Navigation ──────────────────────────

def record_graph_navigation():
    """Record Up/Down arrow navigation through the task graph."""
    cast_file = os.path.join(RECORDINGS_DIR, "tui-graph-nav-raw.cast")
    log("=== Recording 1: Graph Navigation ===")

    with RecordingHarness(
        cast_file=cast_file,
        cwd=DEMO_DIR,
        shell_command=make_shell_cmd(),
        idle_time_limit=3.0,
    ) as h:
        h.wait_for("$", timeout=5)
        h.sleep(0.3)

        if not launch_tui(h):
            return False

        # Let viewer orient — see the graph
        h.sleep(2)
        h.flush_frame()

        # Navigate down through tasks — each press shows ↓ in HUD
        log("Navigating down through tasks...")
        for i in range(6):
            h.send_keys("Down")
            h.sleep(0.8)
            h.flush_frame()

        # Pause to show current selection
        h.sleep(1.5)

        # Navigate back up — HUD shows ↑
        log("Navigating back up...")
        for i in range(4):
            h.send_keys("Up")
            h.sleep(0.8)
            h.flush_frame()

        h.sleep(1.5)
        h.flush_frame()

        exit_tui(h)

        duration = h.duration
        log(f"Graph navigation: {duration:.1f}s, {h.frame_count} frames")

    _verify_cast(cast_file)
    return cast_file


# ── Recording 2: Inspector ──────────────────────────────────

def record_inspector():
    """Record opening/closing inspector and exploring task details."""
    cast_file = os.path.join(RECORDINGS_DIR, "tui-inspector-raw.cast")
    log("=== Recording 2: Inspector ===")

    with RecordingHarness(
        cast_file=cast_file,
        cwd=DEMO_DIR,
        shell_command=make_shell_cmd(),
        idle_time_limit=3.0,
    ) as h:
        h.wait_for("$", timeout=5)
        h.sleep(0.3)

        if not launch_tui(h):
            return False

        h.sleep(1.5)

        # Navigate to an interesting task first
        h.send_keys("Down")
        h.sleep(0.6)
        h.send_keys("Down")
        h.sleep(0.6)
        h.flush_frame()

        # Switch to Detail tab — HUD shows "1"
        log("Detail tab (1)")
        h.send_keys("1")
        h.sleep(2)
        h.flush_frame()

        # Switch to Log tab — HUD shows "2"
        log("Log tab (2)")
        h.send_keys("2")
        h.sleep(2)
        h.flush_frame()

        # Switch to Agency tab — HUD shows "4"
        log("Agency tab (4)")
        h.send_keys("4")
        h.sleep(1.5)
        h.flush_frame()

        # Navigate to a different task to show detail updating
        h.send_keys("Down")
        h.sleep(0.8)
        h.send_keys("Down")
        h.sleep(0.8)
        h.flush_frame()

        # Back to Detail tab
        h.send_keys("1")
        h.sleep(1.5)
        h.flush_frame()

        exit_tui(h)

        duration = h.duration
        log(f"Inspector: {duration:.1f}s, {h.frame_count} frames")

    _verify_cast(cast_file)
    return cast_file


# ── Recording 3: Agent Monitoring ───────────────────────────

def record_agent_monitoring():
    """Record watching agent activity via Log and Firehose tabs."""
    cast_file = os.path.join(RECORDINGS_DIR, "tui-agent-monitor-raw.cast")
    log("=== Recording 3: Agent Monitoring ===")

    with RecordingHarness(
        cast_file=cast_file,
        cwd=DEMO_DIR,
        shell_command=make_shell_cmd(),
        idle_time_limit=3.0,
    ) as h:
        h.wait_for("$", timeout=5)
        h.sleep(0.3)

        if not launch_tui(h):
            return False

        h.sleep(1.5)

        # Navigate to an in-progress task (analyze-mood is index ~1)
        h.send_keys("Down")
        h.sleep(0.6)
        h.flush_frame()

        # Show Log tab for this task — HUD shows "2"
        log("Log tab (2) for in-progress task")
        h.send_keys("2")
        h.sleep(2.5)
        h.flush_frame()

        # Switch to Firehose tab — HUD shows "8"
        log("Firehose tab (8)")
        h.send_keys("8")
        h.sleep(2.5)
        h.flush_frame()

        # Navigate to another in-progress task
        h.send_keys("Down")
        h.sleep(0.6)
        h.send_keys("Down")
        h.sleep(0.6)
        h.send_keys("Down")
        h.sleep(0.6)
        h.flush_frame()

        # Show its Log tab
        h.send_keys("2")
        h.sleep(2)
        h.flush_frame()

        # Back to Chat/overview
        h.send_keys("0")
        h.sleep(1.5)
        h.flush_frame()

        exit_tui(h)

        duration = h.duration
        log(f"Agent monitoring: {duration:.1f}s, {h.frame_count} frames")

    _verify_cast(cast_file)
    return cast_file


# ── Recording 4: Key Workflows ──────────────────────────────

def record_key_workflows():
    """Record common keyboard workflows: chat, navigate, inspect."""
    cast_file = os.path.join(RECORDINGS_DIR, "tui-key-workflows-raw.cast")
    log("=== Recording 4: Key Workflows ===")

    with RecordingHarness(
        cast_file=cast_file,
        cwd=DEMO_DIR,
        shell_command=make_shell_cmd(),
        idle_time_limit=3.0,
    ) as h:
        h.wait_for("$", timeout=5)
        h.sleep(0.3)

        if not launch_tui(h):
            return False

        h.sleep(1.5)

        # Workflow: Enter chat mode — HUD shows "c"
        log("Chat mode (c)")
        h.send_keys("c")
        h.sleep(1)
        h.flush_frame()

        # Type a short message
        h.type_naturally("Add a roast mode", wpm=55)
        h.sleep(0.5)
        h.flush_frame()

        # Escape back to graph — HUD shows "Esc"
        log("Escape back to graph")
        h.send_keys("Escape")
        h.sleep(1)
        h.flush_frame()

        # Navigate down — HUD shows ↓↓↓
        for _ in range(3):
            h.send_keys("Down")
            h.sleep(0.5)

        # Tab to switch panel focus — HUD shows "Tab"
        log("Tab to switch focus")
        h.send_keys("Tab")
        h.sleep(1)
        h.flush_frame()

        # Detail tab — HUD shows "1"
        h.send_keys("1")
        h.sleep(1.5)
        h.flush_frame()

        # Tab back to graph
        h.send_keys("Tab")
        h.sleep(0.8)

        # Navigate more
        h.send_keys("Down")
        h.sleep(0.5)
        h.send_keys("Down")
        h.sleep(0.5)
        h.flush_frame()

        h.sleep(1.5)

        exit_tui(h)

        duration = h.duration
        log(f"Key workflows: {duration:.1f}s, {h.frame_count} frames")

    _verify_cast(cast_file)
    return cast_file


# ── Main ────────────────────────────────────────────────────

def record_all():
    """Record all 4 interaction examples."""
    global _start_time
    _start_time = time.monotonic()

    log("=== TUI Interaction Examples Recording ===")

    # Setup
    setup_demo()

    results = {}

    # Record each interaction
    for name, func in [
        ("graph-navigation", record_graph_navigation),
        ("inspector", record_inspector),
        ("agent-monitoring", record_agent_monitoring),
        ("key-workflows", record_key_workflows),
    ]:
        try:
            result = func()
            results[name] = result
            log(f"  {name}: {'OK' if result else 'FAILED'}")
        except Exception as e:
            log(f"  {name}: ERROR — {e}")
            results[name] = False

    # Summary
    elapsed = time.monotonic() - _start_time
    log(f"\n{'=' * 60}")
    log(f"Total time: {elapsed:.1f}s")
    for name, result in results.items():
        marker = "+" if result else "-"
        log(f"  {marker} {name}: {result}")
    log(f"{'=' * 60}")

    ok = all(results.values())
    return ok


if __name__ == "__main__":
    success = record_all()
    sys.exit(0 if success else 1)
