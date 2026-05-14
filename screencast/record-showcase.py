#!/usr/bin/env python3
"""Record the showcase screencast: 6-scene TUI session with real agents.

Uses the recording harness at 65x38 to capture a real multi-scene
WG session demonstrating the full journey of the haiku-news project.

Scenes:
1. Launch: wg service start → wg tui
2. Fan-out: watch parallel agents spawn
3. Task inspection: arrow through tasks, detail tabs
4. Completions + FLIP: watch eval pipeline
5. Coordinator conversation: type message, get response
6. Second wave: new tasks get worked on

Output: screencast/recordings/showcase-raw.cast
"""

import os
import re
import sys
import time
import json
import subprocess

# Import the harness (file has hyphen in name, can't use regular import)
import importlib.util
_harness_path = os.path.join(os.path.dirname(os.path.abspath(__file__)), "record-harness.py")
_spec = importlib.util.spec_from_file_location("record_harness", _harness_path)
_mod = importlib.util.module_from_spec(_spec)
_spec.loader.exec_module(_mod)
RecordingHarness = _mod.RecordingHarness

DEMO_DIR = os.environ.get("SHOWCASE_DEMO_DIR", "/tmp/haiku-news-pNtaQ5")
CAST_FILE = os.path.join(
    os.path.dirname(os.path.abspath(__file__)),
    "recordings",
    "showcase-raw.cast",
)

# Task IDs in the expected tree order (top to bottom in TUI)
INITIAL_TASKS = [
    "scrape-headlines",
    "analyze-mood",
    "wire-haiku-engine",
    "draft-haikus",
    "review-quality",
    "publish-api",
    "count-syllables",
    "build-pun-db",
]

# Scene tracking
scenes_captured = {}


def log(msg):
    """Print timestamped log message."""
    elapsed = time.monotonic() - _start_time if _start_time else 0
    print(f"[{elapsed:7.1f}s] {msg}", file=sys.stderr)


_start_time = None


def wg_cmd(*args, cwd=None):
    """Run a wg command and return stdout."""
    try:
        r = subprocess.run(
            ["wg"] + list(args),
            capture_output=True, text=True, timeout=10,
            cwd=cwd or DEMO_DIR,
        )
        return r.stdout.strip()
    except Exception as e:
        log(f"wg command failed: {e}")
        return ""


def get_task_statuses():
    """Get current task statuses as a dict."""
    output = wg_cmd("list", "--json")
    if not output:
        # Fallback: parse text output
        output = wg_cmd("list")
        statuses = {}
        for line in output.split("\n"):
            line = line.strip()
            if not line:
                continue
            # Parse lines like "[ ] task-id - Title" or "[●] task-id - Title"
            if "done" in line.lower():
                # Try to extract task id
                pass
        return statuses

    try:
        tasks = json.loads(output)
        return {t["id"]: t.get("status", "open") for t in tasks}
    except (json.JSONDecodeError, TypeError):
        return {}


def wait_for_task_status(task_id, target_status, timeout=300, interval=5):
    """Wait for a task to reach a target status."""
    deadline = time.monotonic() + timeout
    while time.monotonic() < deadline:
        output = wg_cmd("show", task_id)
        if target_status in output.lower():
            return True
        time.sleep(interval)
    return False


def count_tasks_with_status(status_str):
    """Count tasks matching a status string in wg list output."""
    output = wg_cmd("list")
    count = 0
    for line in output.split("\n"):
        if status_str.lower() in line.lower():
            count += 1
    return count


def inject_fallback_tasks():
    """Inject the roast-mode tasks if coordinator didn't create them."""
    log("Injecting fallback roast-mode tasks")
    wg_cmd("add", "Build snark filter", "--id", "build-snark-filter",
           "--after", "count-syllables",
           "-d", "Tone adjustment: convert neutral → snarky/sarcastic. "
           "Reuse syllable counter for 5-7-5 compliance.")
    wg_cmd("add", "Draft roast haikus", "--id", "draft-roast-haikus",
           "--after", "build-snark-filter,wire-haiku-engine",
           "-d", "Generate snarky haiku variants of each headline. "
           "Funny, not mean. Sarcastic, not bitter.")
    wg_cmd("add", "Review roasts", "--id", "review-roasts",
           "--after", "draft-roast-haikus",
           "-d", "Quality gate: funny? clever? still references news? "
           "Flag anything that crosses from snarky to cruel.")


def inject_fallback_chat():
    """Inject fallback chat history for coordinator conversation."""
    log("Injecting fallback chat history")
    chat_path = os.path.join(DEMO_DIR, ".workgraph", "service", "coordinator-chat.json")
    chat_data = [
        {
            "role": "user",
            "text": "Headlines are boring. Add a roast mode.",
            "timestamp": "2026-03-23T20:05:00+00:00",
            "edited": False,
        },
        {
            "role": "assistant",
            "text": ("Adding a roast pipeline — snarky haikus that mock the headlines:\n\n"
                     "1. **build-snark-filter** → tone adjuster (after count-syllables)\n"
                     "2. **draft-roast-haikus** → snarky versions (after snark-filter + haiku-engine)\n"
                     "3. **review-roasts** → quality gate\n\n"
                     "Creating tasks now..."),
            "timestamp": "2026-03-23T20:05:15+00:00",
            "edited": False,
        },
    ]
    os.makedirs(os.path.dirname(chat_path), exist_ok=True)
    with open(chat_path, "w") as f:
        json.dump(chat_data, f, indent=2)


def inject_haiku_logs():
    """Inject sample haiku content into task logs for visual payoff."""
    log("Injecting sample haiku logs")
    wg_cmd("log", "draft-haikus",
           "Generating haiku for headline #3:\n"
           "  'Stock Market Plunges After Fed Rate Decision'\n"
           "  Mood: grim\n\n"
           "  Draft:\n"
           "    Markets tumble down\n"
           "    The Fed says rates will go up\n"
           "    My wallet says ow\n\n"
           "  ✓ Syllable check: 5-7-5 passed\n"
           "  ✓ Topic reference: markets, Fed ✓\n"
           "  ✓ Wordplay: 'ow' (pain pun) ✓")

    wg_cmd("log", "draft-haikus",
           "Generating haiku for headline #7:\n"
           "  'NASA Discovers New Exoplanet in Habitable Zone'\n"
           "  Mood: hopeful\n\n"
           "  Draft:\n"
           "    New world found in space\n"
           "    Not too hot and not too cold\n"
           "    Still no WiFi though\n\n"
           "  ✓ Syllable check: 5-7-5 passed\n"
           "  ✓ Topic reference: NASA, exoplanet ✓\n"
           "  ✓ Wordplay: WiFi (modern inconvenience) ✓")


def inject_roast_logs():
    """Inject sample roast haiku content into task logs."""
    log("Injecting sample roast haiku logs")
    wg_cmd("log", "draft-roast-haikus",
           "Generating roast for headline #1:\n"
           "  'Tech Giants Report Record Quarterly Earnings'\n"
           "  Snark level: medium\n\n"
           "  Roast draft:\n"
           "    Billions in profit\n"
           "    Still can't fix the printer though\n"
           "    Shareholders rejoice\n\n"
           "  ✓ Syllable check: 5-7-5 passed\n"
           "  ✓ Snark level: sarcastic (not cruel) ✓\n"
           "  ✓ Topic: tech earnings ✓")


def record_showcase():
    """Main recording function."""
    global _start_time
    _start_time = time.monotonic()

    log(f"Demo project: {DEMO_DIR}")
    log(f"Cast file: {CAST_FILE}")

    # Verify demo project exists
    if not os.path.exists(os.path.join(DEMO_DIR, ".workgraph")):
        log("ERROR: Demo project not found. Set SHOWCASE_DEMO_DIR.")
        sys.exit(1)

    # Build the shell command that will run inside tmux
    shell_env = os.environ.copy()
    shell_cmd = f"cd {DEMO_DIR} && exec bash --norc --noprofile"

    with RecordingHarness(
        cast_file=CAST_FILE,
        cols=65,
        rows=38,
        fps=5,  # Lower FPS for long recording — saves space
        cwd=DEMO_DIR,
        shell_command=shell_cmd,
        idle_time_limit=3.0,
    ) as h:
        # Let shell prompt appear
        h.sleep(1)
        log("Shell ready")

        # ═══════════════════════════════════════════════════════
        # Scene 1: Launch
        # ═══════════════════════════════════════════════════════
        log("=== Scene 1: Launch ===")

        # Set a clean prompt for the recording
        h.send_text("PS1='$ '\n")
        h.sleep(0.5)
        h.send_keys("C-l")  # Clear screen
        h.sleep(0.5)

        # Show the task graph first (orientation)
        h.type_naturally("wg viz", wpm=40)
        h.send_keys("Enter")
        h.sleep(3)  # Let viewer see the graph
        h.flush_frame()

        # Start the service
        h.type_naturally("wg service start", wpm=40)
        h.send_keys("Enter")
        log("Sent: wg service start")

        # Wait for service to show ready
        found = h.wait_for("service", timeout=15, interval=0.5)
        if found:
            log("Service started")
        else:
            log("WARNING: service start output not detected, continuing")
        h.sleep(2)  # Let viewer read service output

        # Launch TUI
        h.type_naturally("wg tui", wpm=40)
        h.send_keys("Enter")
        log("Sent: wg tui")

        # Wait for TUI to render
        found = h.wait_for("Chat", timeout=15, interval=0.5)
        if found:
            log("TUI rendered")
        else:
            log("WARNING: TUI render not detected, continuing")

        h.sleep(3)  # Viewer orients to full graph
        h.flush_frame()
        scenes_captured["scene1_launch"] = True
        log("Scene 1 complete")

        # ═══════════════════════════════════════════════════════
        # Scene 2: Fan-out — watch agents claim tasks
        # ═══════════════════════════════════════════════════════
        log("=== Scene 2: Fan-out ===")

        # Wait for first task to go in-progress
        log("Waiting for agents to start picking up tasks...")

        # Poll for task activity — check TUI screen for "progress" or status changes
        # The TUI should show tasks transitioning. We wait and observe.
        fan_out_start = time.monotonic()
        fan_out_timeout = 300  # 5 minutes max for fan-out phase

        # Wait for at least one task to show in-progress
        found = h.wait_for("in-progress", timeout=120, interval=2)
        if found:
            log("First task in-progress detected in TUI")
        else:
            log("WARNING: No in-progress detected after 120s")

        h.sleep(5)  # Let more tasks start
        h.flush_frame()

        # Now wait for some tasks to complete
        # Check periodically via wg command outside the TUI
        log("Waiting for initial tasks to complete...")
        wave1_done = False
        deadline = time.monotonic() + fan_out_timeout

        while time.monotonic() < deadline:
            # Check status outside the TUI
            output = wg_cmd("list")
            done_count = output.lower().count("done")
            progress_count = output.lower().count("in-progress")
            log(f"  Status: {done_count} done, {progress_count} in-progress")

            # Capture frames while waiting
            h.sleep(5)

            # Check if scrape-headlines is done (enables fan-out)
            if "done" in wg_cmd("show", "scrape-headlines").lower():
                log("scrape-headlines done — fan-out should be happening")
                h.sleep(10)  # Let the fan-out play out visually
                break

            if done_count >= 2:
                log("Multiple tasks done — fan-out in progress")
                break

        # Continue waiting for more completions
        h.sleep(10)
        h.flush_frame()

        # Navigate to show edge highlighting
        log("Navigating to show dependency edges")
        h.send_keys("Down")  # Move down in task list
        h.sleep(1.5)
        h.flush_frame()
        h.send_keys("Down")
        h.sleep(1.5)
        h.flush_frame()

        # Pause to show edge highlighting
        h.sleep(2)
        h.flush_frame()

        scenes_captured["scene2_fanout"] = True
        log("Scene 2 complete")

        # ═══════════════════════════════════════════════════════
        # Scene 3: Task Inspection
        # ═══════════════════════════════════════════════════════
        log("=== Scene 3: Task Inspection ===")

        # Switch to Detail tab (key: 1)
        h.send_keys("1")
        h.sleep(2)
        h.flush_frame()
        log("Switched to Detail tab")

        # Navigate down to see different tasks
        h.send_keys("Down")
        h.sleep(1.5)
        h.flush_frame()

        # Switch to Log tab (key: 2)
        h.send_keys("2")
        h.sleep(2)
        h.flush_frame()
        log("Switched to Log tab")

        # Scroll through log
        h.send_keys("Down")
        h.sleep(0.8)
        h.send_keys("Down")
        h.sleep(0.8)
        h.send_keys("Down")
        h.sleep(0.8)
        h.flush_frame()

        # Pause to read log content (potential haiku content)
        h.sleep(3)
        h.flush_frame()

        # Navigate up to see completed tasks
        h.send_keys("Up")
        h.sleep(1)
        h.send_keys("Up")
        h.sleep(1)
        h.send_keys("Up")
        h.sleep(1)
        h.send_keys("Up")
        h.sleep(1)
        h.flush_frame()

        # Switch to Output tab (key: 3)
        h.send_keys("3")
        h.sleep(2)
        h.flush_frame()
        log("Switched to Output tab")

        # Pause for viewer
        h.sleep(2)

        # Navigate up to scrape-headlines (should be done)
        h.send_keys("Up")
        h.sleep(1.5)
        h.flush_frame()

        scenes_captured["scene3_inspection"] = True
        log("Scene 3 complete")

        # ═══════════════════════════════════════════════════════
        # Scene 4: Completions + FLIP/Evaluate
        # ═══════════════════════════════════════════════════════
        log("=== Scene 4: Completions + FLIP ===")

        # Wait for more tasks to complete — the pipeline should be progressing
        log("Waiting for pipeline tasks to complete...")
        deadline = time.monotonic() + 600  # 10 min max

        while time.monotonic() < deadline:
            output = wg_cmd("list")
            done_count = output.lower().count("done")
            total_lines = len([l for l in output.split("\n") if l.strip()])
            log(f"  Pipeline status: {done_count}/{total_lines} done")

            h.sleep(10)  # Capture frames while waiting

            # Check for wire-haiku-engine completion (key milestone)
            if "done" in wg_cmd("show", "wire-haiku-engine").lower():
                log("wire-haiku-engine done!")
                break

            # Check for significant progress
            if done_count >= 4:
                log(f"Good progress: {done_count} tasks done")
                break

        # Wait for draft-haikus and review-quality
        log("Waiting for draft-haikus and review cycle...")
        deadline2 = time.monotonic() + 600

        while time.monotonic() < deadline2:
            output = wg_cmd("list")
            done_count = output.lower().count("done")
            log(f"  Cycle status: {done_count} done")

            h.sleep(10)

            # Check if publish-api is done (all initial tasks complete)
            if "done" in wg_cmd("show", "publish-api").lower():
                log("publish-api done — all initial tasks complete!")
                break

            # Or at least review-quality done
            if "done" in wg_cmd("show", "review-quality").lower():
                log("review-quality done")
                h.sleep(5)
                break

            if done_count >= 6:
                log(f"Most tasks done: {done_count}")
                break

        h.sleep(5)
        h.flush_frame()

        # Navigate to see FLIP tasks if they exist
        snap = h.snapshot()
        if ".flip" in snap or "flip" in snap.lower():
            log("FLIP tasks visible in TUI")
        else:
            log("No FLIP tasks visible yet")

        # Navigate through the completed pipeline
        h.send_keys("Down")
        h.sleep(1)
        h.send_keys("Down")
        h.sleep(1)
        h.flush_frame()
        h.sleep(2)

        scenes_captured["scene4_completions"] = True
        log("Scene 4 complete")

        # ═══════════════════════════════════════════════════════
        # Scene 5: Coordinator Conversation
        # ═══════════════════════════════════════════════════════
        log("=== Scene 5: Coordinator Conversation ===")

        # Switch to Chat tab (key: 0)
        h.send_keys("0")
        h.sleep(2)
        h.flush_frame()
        log("Switched to Chat tab")

        # Enter chat input mode (key: c)
        h.send_keys("c")
        h.sleep(1)
        h.flush_frame()

        # Type the roast mode message naturally
        message = "Headlines are boring. Add a roast mode."
        log(f"Typing message: {message}")
        h.type_naturally(message, wpm=50)
        h.sleep(1)
        h.flush_frame()

        # Submit the message
        h.send_keys("Enter")
        log("Message submitted, waiting for coordinator response")
        h.flush_frame()

        # Wait for coordinator response — this can take 30-120s
        coordinator_responded = False
        response_deadline = time.monotonic() + 180  # 3 min timeout

        while time.monotonic() < response_deadline:
            snap = h.snapshot()
            # Look for signs of a response
            if "roast" in snap.lower() or "snark" in snap.lower() or "creating" in snap.lower():
                log("Coordinator response detected!")
                coordinator_responded = True
                h.sleep(5)  # Let response finish streaming
                break
            if "build-snark" in snap.lower() or "draft-roast" in snap.lower():
                log("Coordinator created roast tasks!")
                coordinator_responded = True
                h.sleep(3)
                break
            h.sleep(3)

        if not coordinator_responded:
            log("WARNING: Coordinator didn't respond in 3 min, retrying once")
            # Try pressing Enter again or re-entering
            h.send_keys("Escape")
            h.sleep(1)
            h.send_keys("c")
            h.sleep(1)
            h.type_naturally("Add roast mode please", wpm=50)
            h.send_keys("Enter")

            retry_deadline = time.monotonic() + 180
            while time.monotonic() < retry_deadline:
                snap = h.snapshot()
                if any(kw in snap.lower() for kw in ["roast", "snark", "creating", "build-snark"]):
                    log("Coordinator responded on retry!")
                    coordinator_responded = True
                    h.sleep(5)
                    break
                h.sleep(3)

        if not coordinator_responded:
            log("WARNING: Coordinator did not respond after retry. Using fallback.")
            # Inject the tasks manually
            inject_fallback_tasks()
            h.sleep(5)

        h.flush_frame()
        h.sleep(3)  # Let viewer read the response

        # Exit chat input mode
        h.send_keys("Escape")
        h.sleep(1)

        # Check if new tasks exist
        output = wg_cmd("list")
        has_roast_tasks = "snark" in output.lower() or "roast" in output.lower()
        if not has_roast_tasks:
            log("Roast tasks not found, injecting fallback")
            inject_fallback_tasks()
            h.sleep(3)

        h.flush_frame()
        h.sleep(2)  # Let viewer see expanded graph

        scenes_captured["scene5_coordinator"] = coordinator_responded
        log(f"Scene 5 complete (coordinator responded: {coordinator_responded})")

        # ═══════════════════════════════════════════════════════
        # Scene 6: Second Wave
        # ═══════════════════════════════════════════════════════
        log("=== Scene 6: Second Wave ===")

        # Wait for new tasks to start being worked on
        log("Waiting for second wave tasks to start...")
        wave2_deadline = time.monotonic() + 300  # 5 min

        while time.monotonic() < wave2_deadline:
            output = wg_cmd("list")
            if "snark" in output.lower() or "roast" in output.lower():
                roast_lines = [l for l in output.split("\n")
                              if "snark" in l.lower() or "roast" in l.lower()]
                has_progress = any("in-progress" in l.lower() or "done" in l.lower()
                                  for l in roast_lines)
                if has_progress:
                    log("Second wave tasks in progress/done")
                    break
            h.sleep(5)

        h.sleep(5)
        h.flush_frame()

        # Navigate to the new tasks
        log("Navigating to second wave tasks")
        # Go back to beginning of list first
        for _ in range(8):
            h.send_keys("Up")
            h.sleep(0.3)

        # Navigate down to find the new tasks
        for i in range(12):
            h.send_keys("Down")
            h.sleep(1)
            snap = h.snapshot()
            if "snark" in snap.lower() or "roast" in snap.lower():
                log(f"Found roast task at position {i}")
                break
            h.flush_frame()

        # Show edge highlighting on new task
        h.sleep(2)
        h.flush_frame()

        # Switch to Log tab to see roast content
        h.send_keys("2")
        h.sleep(2)
        h.flush_frame()
        log("Viewing roast task log")

        # Wait for roast tasks to complete
        log("Waiting for second wave to complete...")
        wave2_done_deadline = time.monotonic() + 300

        while time.monotonic() < wave2_done_deadline:
            output = wg_cmd("list")
            roast_lines = [l for l in output.split("\n")
                          if "snark" in l.lower() or "roast" in l.lower()]
            done_roast = sum(1 for l in roast_lines if "done" in l.lower())
            log(f"  Second wave: {done_roast}/{len(roast_lines)} done")

            h.sleep(10)

            if done_roast >= 2:  # At least 2 of 3 roast tasks done
                log("Most second wave tasks done")
                break

        h.sleep(3)
        h.flush_frame()

        # Navigate down to see draft-roast-haikus log
        h.send_keys("Down")
        h.sleep(1.5)
        h.send_keys("2")  # Log tab
        h.sleep(2)
        h.flush_frame()

        # THE payoff — pause for viewer to read snarky haikus
        h.sleep(4)
        h.flush_frame()

        # Survey full graph — navigate up
        log("Surveying full completed graph")
        for _ in range(6):
            h.send_keys("Up")
            h.sleep(0.8)

        h.sleep(3)  # Let viewer see all tasks done
        h.flush_frame()

        scenes_captured["scene6_secondwave"] = True
        log("Scene 6 complete")

        # ═══════════════════════════════════════════════════════
        # Exit
        # ═══════════════════════════════════════════════════════
        log("=== Exit ===")
        h.send_keys("q")
        h.sleep(2)
        h.flush_frame()

        # Final stats
        duration = h.duration
        frames = h.frame_count
        log(f"Recording complete: {duration:.1f}s, {frames} frames")

    return duration, frames


def verify_cast():
    """Verify the cast file meets all requirements."""
    print("\n=== Verifying cast file ===", file=sys.stderr)

    if not os.path.exists(CAST_FILE):
        print(f"ERROR: Cast file not found at {CAST_FILE}", file=sys.stderr)
        return False

    with open(CAST_FILE) as f:
        lines = f.readlines()

    if len(lines) < 2:
        print("ERROR: Cast file has fewer than 2 lines", file=sys.stderr)
        return False

    # Check header
    header = json.loads(lines[0])
    w, h = header.get("width"), header.get("height")
    print(f"  Header: width={w}, height={h}", file=sys.stderr)

    ok = True
    if w != 65 or h != 38:
        print(f"  ERROR: Expected 65x38, got {w}x{h}", file=sys.stderr)
        ok = False

    # Check CR+LF
    bare_lf = 0
    crlf = 0
    for line in lines[1:]:
        try:
            event = json.loads(line)
            if len(event) >= 3 and event[1] == "o":
                data = event[2]
                bare_lf += len(re.findall(r'(?<!\r)\n', data))
                crlf += data.count('\r\n')
        except json.JSONDecodeError:
            pass

    print(f"  Line endings: {crlf} CR+LF, {bare_lf} bare LF", file=sys.stderr)
    if bare_lf > 0:
        print(f"  WARNING: {bare_lf} bare LF occurrences", file=sys.stderr)

    frame_count = len(lines) - 1
    last_event = json.loads(lines[-1])
    duration = last_event[0] if isinstance(last_event, list) else 0
    print(f"  Frames: {frame_count}, Duration: {duration:.1f}s", file=sys.stderr)

    return ok


if __name__ == "__main__":
    print("=" * 60, file=sys.stderr)
    print("  SHOWCASE SCREENCAST RECORDING", file=sys.stderr)
    print("=" * 60, file=sys.stderr)

    duration, frames = record_showcase()

    print("\n" + "=" * 60, file=sys.stderr)
    print("  RECORDING SUMMARY", file=sys.stderr)
    print("=" * 60, file=sys.stderr)
    print(f"  Duration: {duration:.1f}s ({duration/60:.1f} min)", file=sys.stderr)
    print(f"  Frames: {frames}", file=sys.stderr)
    print(f"  Cast file: {CAST_FILE}", file=sys.stderr)
    print(f"  Scenes captured:", file=sys.stderr)
    for scene, status in scenes_captured.items():
        marker = "✓" if status else "✗"
        print(f"    {marker} {scene}", file=sys.stderr)
    print("=" * 60, file=sys.stderr)

    verify_cast()
