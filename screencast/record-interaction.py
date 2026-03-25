#!/usr/bin/env python3
"""Record the interaction screencast: CLI orient → TUI → coordinator → agents → live output.

The new "hero" screencast that leads with CLI context before the TUI.
Based on the design in docs/design/screencast-interaction-flow.md.

Scenes:
0. CLI Orient: wg status, wg list, wg ready, wg viz — establish CLI workflow
1. Launch: wg tui (service pre-started)
2. Talk to Coordinator: type prompt, coordinator responds + creates tasks
3. Tasks Appear + Agents Spawn: graph fills in, parallel execution
4. Live Detail View: Detail → Agency → Firehose (log) tabs showing agent output
5. Conversation Round 2: follow-up message, coordinator adapts graph
6. Final Survey + Exit: review completed tasks, quit

Output: screencast/recordings/interaction-raw.cast
"""

import json
import os
import random
import re
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
CAST_FILE = os.path.join(SCRIPT_DIR, "recordings", "interaction-raw.cast")
DEMO_DIR = f"/tmp/wg-interaction-{os.getpid()}"

# Main prompt — the "haiku news pipeline" scenario
PROMPT = "Build a haiku news pipeline — scrape headlines, generate haiku for each, and publish an API"

# Follow-up prompt
FOLLOWUP = "Headlines are boring. Add a roast mode."

# CLAUDE.md for the demo coordinator
CLAUDE_MD = """\
# Haiku News Pipeline Demo

When the user asks to build a haiku news pipeline, decompose into these tasks:

1. scrape-headlines — Fetch top news headlines from RSS feeds
2. analyze-mood — Sentiment analysis on each headline (after scrape-headlines)
3. count-syllables — Build syllable-counting engine (parallel with scrape-headlines)
4. build-pun-db — Collect wordplay database (parallel with scrape-headlines)
5. wire-haiku-engine — Core haiku generator (after scrape-headlines, count-syllables)
6. draft-haikus — Generate haiku for each headline (after wire-haiku-engine, analyze-mood)
7. review-quality — Quality gate on generated haiku (after draft-haikus)
8. publish-api — REST API serving approved haiku (after review-quality, build-pun-db)

Use exactly these task IDs. Create all 8 tasks using wg add with --after dependencies.
Tasks scrape-headlines, count-syllables, and build-pun-db start in parallel.
Keep your response brief (just list the tasks and their dependencies).
Do NOT create any other tasks or subtasks.

When the user asks to "add a roast mode":
1. build-snark-filter — Tone adjuster: neutral → snarky (after count-syllables)
2. draft-roast-haikus — Snarky haiku variants (after build-snark-filter, wire-haiku-engine)
3. review-roasts — Quality gate for roast haiku (after draft-roast-haikus)

Use exactly these IDs. Keep response brief.
Do NOT create any tasks until the user sends a chat message.
"""

# Initial task IDs we expect from the coordinator
INITIAL_TASK_IDS = {
    "scrape-headlines", "analyze-mood", "count-syllables", "build-pun-db",
    "wire-haiku-engine", "draft-haikus", "review-quality", "publish-api",
}

# Roast task IDs for scene 5
ROAST_TASK_IDS = {
    "build-snark-filter", "draft-roast-haikus", "review-roasts",
}

# Fallback task definitions
TASKS_FALLBACK = [
    ("Scrape headlines", "scrape-headlines", None,
     "Fetch top news headlines from RSS feeds"),
    ("Analyze mood", "analyze-mood", "scrape-headlines",
     "Sentiment analysis on each headline"),
    ("Count syllables", "count-syllables", None,
     "Build syllable-counting engine"),
    ("Build pun db", "build-pun-db", None,
     "Collect wordplay database for haiku generation"),
    ("Wire haiku engine", "wire-haiku-engine", "scrape-headlines,count-syllables",
     "Core haiku generator combining headlines + syllable rules"),
    ("Draft haikus", "draft-haikus", "wire-haiku-engine,analyze-mood",
     "Generate haiku for each headline using mood + engine"),
    ("Review quality", "review-quality", "draft-haikus",
     "Quality gate on generated haiku — syllable counts + topic relevance"),
    ("Publish API", "publish-api", "review-quality,build-pun-db",
     "REST API serving approved haiku with pun database enrichment"),
]

ROAST_FALLBACK = [
    ("Build snark filter", "build-snark-filter", "count-syllables",
     "Tone adjustment: convert neutral to snarky/sarcastic"),
    ("Draft roast haikus", "draft-roast-haikus", "build-snark-filter,wire-haiku-engine",
     "Generate snarky haiku variants of each headline"),
    ("Review roasts", "review-roasts", "draft-roast-haikus",
     "Quality gate: funny but not cruel"),
]

CHAT_RESPONSE_FALLBACK = (
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
)

ROAST_RESPONSE_FALLBACK = (
    "Adding roast mode:\n\n"
    "1. **build-snark-filter** — tone adjuster (after syllables)\n"
    "2. **draft-roast-haikus** — snarky variants (after snark + engine)\n"
    "3. **review-roasts** — quality gate\n\n"
    "Creating tasks now..."
)

# Scene tracking
scenes_captured = {}
_start_time = None


def log(msg):
    """Print timestamped log message."""
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


def check_tasks_exist(expected_ids, timeout=180):
    """Wait for expected tasks to appear in the graph."""
    deadline = time.monotonic() + timeout
    last_report = 0

    while time.monotonic() < deadline:
        r = wg("list")
        if r and r.stdout:
            found = {tid for tid in expected_ids if tid in r.stdout}
            now = time.monotonic()
            if found == expected_ids:
                log(f"  All {len(expected_ids)} expected tasks created!")
                return True
            if now - last_report > 15:
                log(f"  {len(found)}/{len(expected_ids)} tasks ({int(deadline - now)}s remaining)")
                last_report = now
        time.sleep(3)

    log(f"  TIMEOUT: not all tasks created in {timeout}s")
    return False


def inject_initial_tasks():
    """Inject the initial haiku pipeline tasks as fallback."""
    log("Injecting fallback initial tasks")
    for title, tid, after, desc in TASKS_FALLBACK:
        cmd = ["add", title, "--id", tid, "-d", desc]
        if after:
            cmd.extend(["--after", after])
        wg(*cmd)
        time.sleep(0.3)


def inject_roast_tasks():
    """Inject the roast-mode tasks as fallback."""
    log("Injecting fallback roast tasks")
    for title, tid, after, desc in ROAST_FALLBACK:
        cmd = ["add", title, "--id", tid, "-d", desc]
        if after:
            cmd.extend(["--after", after])
        wg(*cmd)
        time.sleep(0.3)


def inject_chat_history(entries):
    """Write chat history to coordinator chat file."""
    chat_path = os.path.join(DEMO_DIR, ".workgraph", "chat-history.json")
    os.makedirs(os.path.dirname(chat_path), exist_ok=True)
    with open(chat_path, "w") as f:
        json.dump(entries, f, indent=2)


def count_alive_agents():
    """Count currently alive agents."""
    r = wg("agents", "--alive")
    if r and r.stdout:
        lines = [l for l in r.stdout.strip().split("\n") if l.strip() and "agent" in l.lower()]
        return len(lines)
    return 0


def setup_demo():
    """Initialize a fresh demo project."""
    if os.path.exists(DEMO_DIR):
        subprocess.run(["rm", "-rf", DEMO_DIR])
    os.makedirs(DEMO_DIR)

    subprocess.run(["git", "init", "-q"], cwd=DEMO_DIR, check=True)
    subprocess.run(
        ["git", "commit", "--allow-empty", "-m", "init", "-q"],
        cwd=DEMO_DIR, check=True,
    )

    wg("init")

    # Write CLAUDE.md
    with open(os.path.join(DEMO_DIR, "CLAUDE.md"), "w") as f:
        f.write(CLAUDE_MD)

    # Configure service
    wg("config", "--max-agents", "4")
    wg("config", "--model", "sonnet")
    wg("config", "--coordinator-executor", "claude")

    # Set coordinator model in config.toml
    config_path = os.path.join(DEMO_DIR, ".workgraph", "config.toml")
    with open(config_path) as f:
        config = f.read()

    # Add coordinator model if section exists
    if "[coordinator]" in config:
        config = config.replace("[coordinator]", '[coordinator]\nmodel = "sonnet"', 1)

    # Hide system tasks for cleaner display
    if "show_system_tasks" in config:
        config = config.replace("show_system_tasks = true", "show_system_tasks = false")

    with open(config_path, "w") as f:
        f.write(config)

    log(f"Demo project at {DEMO_DIR}")


def start_service():
    """Start the wg service."""
    wg("service", "start", "--force")
    time.sleep(3)

    r = wg("service", "status")
    if r and r.stdout:
        for line in r.stdout.strip().split("\n")[:2]:
            log(f"  {line}")


# ── Scenes ──────────────────────────────────────────────────

def scene_0_cli(h):
    """Scene 0: CLI Orient — show key commands before entering TUI."""
    log("=== Scene 0: CLI Orient ===")

    h.wait_for("$", timeout=5)
    h.send_keys("C-l")
    h.sleep(0.5)

    # wg status — show project state
    h.type_naturally("wg status", wpm=45)
    h.send_keys("Enter")
    log("Sent: wg status")
    h.sleep(2.5)
    h.flush_frame()

    # wg list — show task list (empty at start)
    h.type_naturally("wg list", wpm=45)
    h.send_keys("Enter")
    log("Sent: wg list")
    h.sleep(2)
    h.flush_frame()

    # wg ready — what's available to work on
    h.type_naturally("wg ready", wpm=45)
    h.send_keys("Enter")
    log("Sent: wg ready")
    h.sleep(2)
    h.flush_frame()

    # wg viz — ASCII dependency graph
    h.type_naturally("wg viz", wpm=45)
    h.send_keys("Enter")
    log("Sent: wg viz")
    h.sleep(2.5)
    h.flush_frame()

    scenes_captured["scene0_cli"] = True
    log("Scene 0 complete")


def scene_1_launch(h):
    """Scene 1: Launch TUI (service already running)."""
    log("=== Scene 1: Launch + Orient ===")

    # Clear screen for clean transition to TUI
    h.send_keys("C-l")
    h.sleep(0.3)

    # Type wg tui naturally
    h.type_naturally("wg tui", wpm=40)
    h.send_keys("Enter")
    log("Sent: wg tui")

    # Wait for TUI to render
    found = h.wait_for("Chat", timeout=15, interval=0.5)
    if found:
        log("TUI rendered")
    else:
        log("WARNING: TUI render not detected")

    # Let viewer see the default layout first
    h.sleep(1.5)
    h.flush_frame()

    # Shrink inspector to give graph more visible space.
    # Uppercase I = shrink_viz_pane() which decreases right_panel_percent,
    # making the inspector smaller and the graph larger.
    for _ in range(3):
        h.send_keys("I")
        h.sleep(0.5)
    h.flush_frame()
    log("Shrunk inspector panel (Shift+I x3)")

    # Let viewer orient to the new layout (graph prominent, smaller inspector)
    h.sleep(3)
    h.flush_frame()

    scenes_captured["scene1_launch"] = found
    log("Scene 1 complete")
    return found


def scene_2_chat(h, use_real_coordinator=True):
    """Scene 2: Talk to Coordinator — type prompt, get response + tasks."""
    log("=== Scene 2: Talk to Coordinator ===")

    # Enter chat input mode
    h.send_keys("c")
    h.sleep(1)
    h.flush_frame()

    # Type the prompt naturally
    log(f"Typing: {PROMPT}")
    h.type_naturally(PROMPT, wpm=50)
    h.sleep(0.5)
    h.flush_frame()

    # Submit
    h.send_keys("Enter")
    log("Message submitted, waiting for coordinator")
    h.flush_frame()

    coordinator_ok = False

    if use_real_coordinator:
        # Wait for coordinator to create tasks
        coordinator_ok = check_tasks_exist(INITIAL_TASK_IDS, timeout=180)

        if not coordinator_ok:
            log("Coordinator failed — using fallback")
            inject_initial_tasks()
            inject_chat_history([
                {
                    "role": "user",
                    "text": PROMPT,
                    "timestamp": "2026-03-24T15:00:01+00:00",
                    "edited": False,
                },
                {
                    "role": "assistant",
                    "text": CHAT_RESPONSE_FALLBACK,
                    "timestamp": "2026-03-24T15:00:10+00:00",
                    "edited": False,
                },
            ])
    else:
        time.sleep(2)
        inject_initial_tasks()
        inject_chat_history([
            {
                "role": "user",
                "text": PROMPT,
                "timestamp": "2026-03-24T15:00:01+00:00",
                "edited": False,
            },
            {
                "role": "assistant",
                "text": CHAT_RESPONSE_FALLBACK,
                "timestamp": "2026-03-24T15:00:10+00:00",
                "edited": False,
            },
        ])
        coordinator_ok = False

    # Let TUI refresh and show response + tasks
    h.sleep(5)
    h.flush_frame()

    # Verify tasks visible
    snap = h.snapshot()
    has_tasks = any(tid in snap for tid in ["scrape-headlines", "wire-haiku", "publish-api"])
    log(f"Tasks visible in TUI: {has_tasks}")

    scenes_captured["scene2_chat"] = coordinator_ok
    log(f"Scene 2 complete (coordinator: {coordinator_ok})")
    return coordinator_ok


def scene_3_agents_spawn(h):
    """Scene 3: Tasks Appear + Agents Spawn — arrow navigation highlights."""
    log("=== Scene 3: Tasks Appear + Agents Spawn ===")

    # Exit chat input, then navigate to graph panel
    h.send_keys("Escape")
    h.sleep(0.8)
    h.send_keys("Tab")
    h.sleep(0.8)
    h.flush_frame()
    log("ESC → TAB: navigated from text entry to graph view")

    # Wait for first task to go in-progress
    log("Waiting for agents to claim tasks...")
    found = h.wait_for("in-progress", timeout=120, interval=2)
    if found:
        log("First task in-progress!")
    else:
        log("WARNING: No in-progress after 120s")

    # Let viewer see tasks appearing and agents spawning
    h.sleep(3)
    h.flush_frame()

    # Navigate down through tasks — each arrow press updates the inspector.
    # Deliberate pacing so the viewer can see the selected node changing
    # and the inspector detail updating for each task.
    log("Arrow key navigation through graph nodes...")
    for i in range(5):
        h.send_keys("Down")
        h.sleep(1.8)
        h.flush_frame()

    # Longer pause so viewer absorbs the selected node + inspector detail
    h.sleep(3)
    h.flush_frame()

    # Navigate back up to show bidirectional navigation
    for i in range(3):
        h.send_keys("Up")
        h.sleep(1.5)
        h.flush_frame()

    h.sleep(2.5)
    h.flush_frame()

    scenes_captured["scene3_agents"] = found
    log("Scene 3 complete")


def scene_4_detail_view(h):
    """Scene 4: Live Detail View — Detail, Log, Firehose showcase.

    This is the most important scene. The viewer sees agents producing
    output in real time and learns to navigate between detail views.
    """
    log("=== Scene 4: Live Detail View ===")

    # Find an in-progress task to inspect
    r = wg("list")
    in_progress_tasks = []
    if r and r.stdout:
        for line in r.stdout.split("\n"):
            if "in-progress" in line.lower():
                parts = line.strip().split()
                if parts:
                    in_progress_tasks.append(parts[0])
    log(f"In-progress tasks: {in_progress_tasks}")

    # Navigate to an in-progress task
    for i in range(6):
        snap = h.snapshot()
        if "in-progress" in snap.lower() or "progress" in snap.lower():
            break
        h.send_keys("Down")
        h.sleep(1)

    # Sub-scene 4a: Detail tab (key 1) — task metadata + live refresh
    log("Sub-scene 4a: Detail tab (key 1)")
    h.send_keys("1")
    h.sleep(4)
    h.flush_frame()

    # Navigate to a different task to show the detail view UPDATING
    # per selection — this demonstrates that arrow keys + detail views
    # work together as a browsing interface
    h.send_keys("Down")
    h.sleep(2)
    h.flush_frame()
    log("Moved to next task — detail view updated")

    h.send_keys("Down")
    h.sleep(2)
    h.flush_frame()
    log("Moved to another task — detail view updated again")

    # Sub-scene 4b: Log tab (key 2) — reverse-chronological activity log
    log("Sub-scene 4b: Log tab (key 2)")
    h.send_keys("2")
    h.sleep(4)
    h.flush_frame()

    # Navigate to yet another task while on Log tab to show it updates
    h.send_keys("Up")
    h.sleep(2)
    h.flush_frame()

    # Sub-scene 4c: Firehose tab (key 8) — THE money shot
    # Merged stream from ALL active agents simultaneously
    log("Sub-scene 4c: Firehose tab (key 8)")
    alive = count_alive_agents()
    log(f"Alive agents: {alive}")

    h.send_keys("8")
    # Extended pause to let the viewer watch log output scrolling
    # as agents produce work in real time. This is the "wow" moment.
    h.sleep(8)
    h.flush_frame()

    if alive >= 1:
        log("Firehose tab shown with live agents — extended viewing pause")
    else:
        log("No alive agents — firehose may be empty")

    # Sub-scene 4d: Back to Detail tab (key 1) to show we can cycle views
    log("Sub-scene 4d: Back to Detail tab (key 1)")
    h.send_keys("1")
    h.sleep(3)
    h.flush_frame()

    scenes_captured["scene4_detail"] = True
    log("Scene 4 complete")


def scene_5_round2(h, use_real_coordinator=True):
    """Scene 5: Conversation Round 2 — follow-up message."""
    log("=== Scene 5: Conversation Round 2 ===")

    # Switch to Chat tab
    h.send_keys("0")
    h.sleep(2)
    h.flush_frame()

    # Enter chat input
    h.send_keys("c")
    h.sleep(1)
    h.flush_frame()

    # Type follow-up naturally
    log(f"Typing: {FOLLOWUP}")
    h.type_naturally(FOLLOWUP, wpm=50)
    h.sleep(0.5)
    h.flush_frame()

    # Submit
    h.send_keys("Enter")
    log("Follow-up submitted, waiting for coordinator")
    h.flush_frame()

    coordinator_ok = False

    if use_real_coordinator:
        # Wait for roast tasks
        coordinator_ok = check_tasks_exist(
            ROAST_TASK_IDS, timeout=180
        )

        if not coordinator_ok:
            log("Coordinator failed on follow-up — using fallback")
            inject_roast_tasks()
    else:
        time.sleep(2)
        inject_roast_tasks()

    # Let TUI refresh and show response + new tasks
    h.sleep(5)
    h.flush_frame()

    # Exit chat input, then navigate to graph panel (ESC → TAB)
    h.send_keys("Escape")
    h.sleep(0.8)
    h.send_keys("Tab")
    h.sleep(0.8)
    h.flush_frame()
    log("ESC → TAB: navigated from text entry to graph view")

    # Navigate down to find the new roast-mode tasks
    for i in range(4):
        h.send_keys("Down")
        h.sleep(1.2)
        h.flush_frame()

    snap = h.snapshot()
    has_roast = "snark" in snap.lower() or "roast" in snap.lower()
    log(f"Roast tasks visible: {has_roast}")

    # Pause so viewer can see the expanded graph with new tasks
    h.sleep(3)
    h.flush_frame()

    scenes_captured["scene5_round2"] = coordinator_ok
    log(f"Scene 5 complete (coordinator: {coordinator_ok})")


def scene_6_survey_exit(h):
    """Scene 6: Final Survey + Exit."""
    log("=== Scene 6: Final Survey + Exit ===")

    # Wait for some second-wave tasks to progress
    log("Waiting for second wave progress...")
    deadline = time.monotonic() + 120

    while time.monotonic() < deadline:
        r = wg("list")
        if r and r.stdout:
            roast_lines = [l for l in r.stdout.split("\n")
                          if "snark" in l.lower() or "roast" in l.lower()]
            has_activity = any("in-progress" in l.lower() or "done" in l.lower()
                             for l in roast_lines)
            if has_activity:
                log("Second wave has activity")
                break
        h.sleep(5)

    h.sleep(3)

    # Navigate to a roast task and show its log
    for i in range(10):
        snap = h.snapshot()
        if "snark" in snap.lower() or "roast" in snap.lower():
            log(f"Found roast task at position {i}")
            break
        h.send_keys("Down")
        h.sleep(1)

    # Show Log tab for the roast task — let viewer read the content
    h.send_keys("2")
    h.sleep(4)
    h.flush_frame()

    # Switch to Detail tab briefly to show completed task output
    h.send_keys("1")
    h.sleep(3)
    h.flush_frame()

    # Navigate back up through the full graph to survey all tasks
    for _ in range(8):
        h.send_keys("Up")
        h.sleep(0.8)
        h.flush_frame()

    # Final pause — let viewer absorb the completed graph
    h.sleep(3)
    h.flush_frame()

    # Exit TUI
    h.send_keys("q")
    h.sleep(2)
    h.flush_frame()

    scenes_captured["scene6_exit"] = True
    log("Scene 6 complete")


# ── Main ────────────────────────────────────────────────────

def record():
    """Main recording orchestrator."""
    global _start_time
    _start_time = time.monotonic()

    log("=== Interaction Screencast Recording ===")
    log(f"Cast file: {CAST_FILE}")

    # Phase 0: Setup
    log("=== Setup ===")
    setup_demo()

    # Pre-start the service (design says service is running before recording)
    start_service()

    # Check if we can use real coordinator
    creds_exist = os.path.exists(os.path.expanduser("~/.claude/.credentials.json"))
    use_real = creds_exist
    log(f"Claude credentials: {'found' if creds_exist else 'not found'}")
    log(f"Coordinator mode: {'real' if use_real else 'simulated fallback'}")

    try:
        shell_cmd = (
            f"cd {DEMO_DIR} && "
            f"export PS1='\\[\\033[1;32m\\]$ \\[\\033[0m\\]' && "
            f"exec bash --norc --noprofile"
        )

        with RecordingHarness(
            cast_file=CAST_FILE,
            cwd=DEMO_DIR,
            shell_command=shell_cmd,
            idle_time_limit=5.0,
        ) as h:
            # Scene 0: CLI Orient
            scene_0_cli(h)

            # Scene 1: Launch
            tui_ok = scene_1_launch(h)
            if not tui_ok:
                log("ERROR: TUI did not load. Aborting.")
                return False

            # Scene 2: Talk to Coordinator
            scene_2_chat(h, use_real_coordinator=use_real)

            # Scene 3: Agents Spawn
            scene_3_agents_spawn(h)

            # Scene 4: Live Detail View
            scene_4_detail_view(h)

            # Scene 5: Conversation Round 2
            scene_5_round2(h, use_real_coordinator=use_real)

            # Scene 6: Final Survey + Exit
            scene_6_survey_exit(h)

            duration = h.duration
            frames = h.frame_count

        # Print summary
        log(f"\n{'=' * 60}")
        log(f"Recording complete: {duration:.1f}s ({duration/60:.1f} min), {frames} frames")
        log(f"Cast file: {CAST_FILE}")
        log(f"Scenes captured:")
        for scene, status in scenes_captured.items():
            marker = "+" if status else "-"
            log(f"  {marker} {scene}")
        log(f"{'=' * 60}")

        # Verify
        log("Verifying cast file...")
        ok = _verify_cast(CAST_FILE)
        return ok

    finally:
        wg("service", "stop")
        log(f"Demo dir: {DEMO_DIR}")


if __name__ == "__main__":
    success = record()
    sys.exit(0 if success else 1)
