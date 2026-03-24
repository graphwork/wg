#!/usr/bin/env python3
"""Record a screencast demonstrating touch/mouse interaction with the TUI.

Shows:
- Click on tasks in the list to select them
- Click on graph nodes to focus them
- Touch echo circles visible at each interaction point
- Mixed mouse clicks with keyboard shortcuts for fluid demo

Output: screencast/recordings/touch-demo-raw.cast
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
CAST_FILE = os.path.join(SCRIPT_DIR, "recordings", "touch-demo-raw.cast")
DEMO_DIR = f"/tmp/wg-touch-demo-{os.getpid()}"

# Task definitions for a realistic graph
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


def send_mouse_click(h, col, row):
    """Send a mouse click (press + release) via SGR escape sequences.

    Uses tmux load-buffer + paste-buffer -r to inject raw bytes into the
    pane's pty without bracketed paste wrapping. The TUI uses SGR extended
    mouse mode (\\x1b[?1006h), so crossterm parses these as Mouse events.

    Args:
        col: 0-based column
        row: 0-based row
    """
    # SGR mouse: 1-based coordinates
    cx = col + 1
    cy = row + 1

    # Build press + release sequences
    press = f"\x1b[<0;{cx};{cy}M"
    release = f"\x1b[<0;{cx};{cy}m"

    # Inject via tmux paste-buffer -r (raw, no bracketed paste)
    import tempfile
    for seq in [press, release]:
        with tempfile.NamedTemporaryFile(mode='wb', suffix='.bin', delete=False) as f:
            f.write(seq.encode())
            tmpf = f.name
        h._tmux("load-buffer", tmpf)
        h._tmux("paste-buffer", "-t", h.session, "-d", "-r")
        os.unlink(tmpf)
        time.sleep(0.05)
        h._capture_frame()
        time.sleep(0.05)


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

    # Add fake log entries
    for tid, entries in FAKE_LOGS.items():
        for entry in entries:
            wg("log", tid, entry)
            time.sleep(0.1)

    # Inject chat history
    chat_path = os.path.join(DEMO_DIR, ".workgraph", "chat-history.json")
    os.makedirs(os.path.dirname(chat_path), exist_ok=True)
    with open(chat_path, "w") as f:
        json.dump([
            {
                "role": "user",
                "text": "Build a haiku news pipeline",
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


def record():
    """Record the touch/mouse interaction demo."""
    global _start_time
    _start_time = time.monotonic()

    log("=== Touch/Mouse Interaction Demo Recording ===")
    log(f"Cast file: {CAST_FILE}")

    # Setup
    setup_demo()

    shell_cmd = (
        f"cd {DEMO_DIR} && "
        f"export PS1='\\[\\033[1;32m\\]$ \\[\\033[0m\\]' && "
        f"exec bash --norc --noprofile"
    )

    with RecordingHarness(
        cast_file=CAST_FILE,
        cwd=DEMO_DIR,
        shell_command=shell_cmd,
        idle_time_limit=2.0,
    ) as h:
        h.wait_for("$", timeout=5)
        h.sleep(0.3)

        # Launch TUI with --show-keys
        h.type_naturally("wg tui --show-keys", wpm=60)
        h.send_keys("Enter")
        found = h.wait_for("Chat", timeout=15, interval=0.5)
        if not found:
            log("ERROR: TUI did not render")
            return False
        log("TUI rendered")
        h.sleep(1)

        # Mouse is enabled by default in single-pane tmux (no split detected).
        # Just enable touch echo (* key) for visual click feedback.
        h.send_keys("*")
        h.sleep(0.3)
        log("Touch echo enabled")
        h.flush_frame()

        # Brief orient pause
        h.sleep(0.5)
        h.flush_frame()

        # ── Click on tasks in the graph list ──
        # Layout: Row 1 = scrape-headlines, Row 2 = ├→ analyze-mood, etc.
        # Click text content to select. Each click shows a touch echo circle.

        # Click scrape-headlines
        send_mouse_click(h, 8, 1)
        h.sleep(0.8)
        h.flush_frame()
        log("Clicked: scrape-headlines")

        # Click wire-haiku-engine (further down)
        send_mouse_click(h, 10, 4)
        h.sleep(0.8)
        h.flush_frame()
        log("Clicked: wire-haiku-engine")

        # ── Mixed input: keyboard + mouse ──
        # Arrow keys to navigate (HUD shows ↓)
        h.send_keys("Down")
        h.sleep(0.4)
        h.send_keys("Down")
        h.sleep(0.4)
        h.flush_frame()
        log("Arrow keys ↓↓")

        # Click publish-api at bottom
        send_mouse_click(h, 10, 8)
        h.sleep(0.8)
        h.flush_frame()
        log("Clicked: publish-api")

        # Arrow back up
        h.send_keys("Up")
        h.sleep(0.4)
        h.send_keys("Up")
        h.sleep(0.4)
        h.flush_frame()
        log("Arrow keys ↑↑")

        # Click analyze-mood
        send_mouse_click(h, 10, 2)
        h.sleep(0.8)
        h.flush_frame()
        log("Clicked: analyze-mood")

        # Keyboard: switch to Detail tab
        h.send_keys("1")
        h.sleep(0.6)
        h.flush_frame()
        log("Keyboard: Detail tab (1)")

        # Click build-pun-db (shows inspector updating)
        send_mouse_click(h, 10, 7)
        h.sleep(0.8)
        h.flush_frame()
        log("Clicked: build-pun-db")

        # Final click on count-syllables
        send_mouse_click(h, 10, 6)
        h.sleep(0.8)
        h.flush_frame()
        log("Clicked: count-syllables")

        # Let last echo fade
        h.sleep(0.5)
        h.flush_frame()

        # Exit TUI
        h.send_keys("q")
        h.sleep(1)

        duration = h.duration
        frames = h.frame_count

    log(f"\nRecording complete: {duration:.1f}s, {frames} frames")
    log(f"Cast file: {CAST_FILE}")

    # Verify
    log("Verifying cast file...")
    ok = _verify_cast(CAST_FILE)
    return ok


if __name__ == "__main__":
    success = record()
    sys.exit(0 if success else 1)
