#!/usr/bin/env python3
"""
Record the hero screencast for the website with live TUI interaction.

All visible interaction happens through keystroke injection into the TUI.
Background CLI handles setup and controlled task progression simulation.

Approach:
  - Disable coordinator agent (prevents interference)
  - Type prompt via keystrokes (visible typing in TUI)
  - Background: inject chat history + create tasks after submission
  - TUI auto-refreshes to show chat response and tasks appearing
  - Navigate tasks via arrow keys, switch inspector tabs
  - Simulate task progression via background wg claim/done
"""

import json
import os
import random
import re
import subprocess
import sys
import time

# -- Configuration ---------------------------------------------------------
COLS = 65
ROWS = 38
FPS = 15

SCRIPT_DIR = os.path.dirname(os.path.abspath(__file__))
DEMO_DIR = f"/tmp/wg-hero-demo-{os.getpid()}"
SESSION = f"wg-hero-{os.getpid()}"
CAST_FILE = os.path.join(SCRIPT_DIR, "recordings", "hero-v2-raw.cast")

PROMPT = "Build a recipe API with auth and search"

TASKS = [
    ("Design API", "design-api", None),
    ("Setup database", "setup-db", "design-api"),
    ("Build auth", "build-auth", "design-api"),
    ("User endpoints", "user-endpoints", "setup-db,build-auth"),
    ("Search API", "search-api", "setup-db"),
    ("Integration tests", "int-tests", "user-endpoints,search-api"),
]

CHAT_RESPONSE = (
    "Breaking this into a task graph:\n\n"
    "1. **design-api** \u2014 REST API schema\n"
    "2. **setup-db** \u2192 database models\n"
    "3. **build-auth** \u2192 JWT auth module\n"
    "4. **user-endpoints** \u2192 CRUD ops\n"
    "5. **search-api** \u2192 search & filter\n"
    "6. **int-tests** \u2192 integration tests\n\n"
    "2-3 and 4-5 run in parallel. Creating now..."
)

# Progression batches: (task_ids, hold_time_seconds)
PROGRESSION = [
    (["design-api"], 3),
    (["setup-db", "build-auth"], 4),
    (["user-endpoints", "search-api"], 3),
    (["int-tests"], 3),
]

random.seed(42)


# -- Helpers ---------------------------------------------------------------

def tmux(*args):
    try:
        return subprocess.run(
            ["tmux"] + list(args),
            capture_output=True, text=True, timeout=10,
        )
    except subprocess.TimeoutExpired:
        return None


def capture_pane():
    r = tmux("capture-pane", "-t", SESSION, "-p")
    return r.stdout if r else ""


def send_keys(*keys):
    tmux("send-keys", "-t", SESSION, *keys)


def send_literal(text):
    tmux("send-keys", "-t", SESSION, "-l", text)


def type_naturally(text):
    for ch in text:
        send_literal(ch)
        delay = random.uniform(0.03, 0.10)
        time.sleep(delay)


def wg_cmd(*args):
    try:
        return subprocess.run(
            ["wg"] + list(args),
            capture_output=True, text=True,
            cwd=DEMO_DIR, timeout=30,
        )
    except subprocess.TimeoutExpired:
        return None


# -- Setup -----------------------------------------------------------------

def setup_demo():
    print(f"Setting up demo at {DEMO_DIR}")
    subprocess.run(["rm", "-rf", DEMO_DIR])
    os.makedirs(DEMO_DIR, exist_ok=True)
    subprocess.run(["git", "init", "-q"], cwd=DEMO_DIR)
    subprocess.run(
        ["git", "commit", "--allow-empty", "-m", "init", "-q"],
        cwd=DEMO_DIR,
    )
    wg_cmd("init")

    # Configure: NO coordinator agent, no task agents
    wg_cmd("config", "--max-agents", "0")

    config_path = os.path.join(DEMO_DIR, ".workgraph", "config.toml")
    with open(config_path) as f:
        config = f.read()

    # Disable coordinator agent (we'll simulate its responses)
    config = config.replace(
        'coordinator_agent = true',
        'coordinator_agent = false',
    )

    # Hide system tasks
    config = config.replace(
        'show_system_tasks = true',
        'show_system_tasks = false',
    )

    with open(config_path, "w") as f:
        f.write(config)

    print("  Config: coordinator_agent=false, max_agents=0, system_tasks=hidden")


def start_service():
    wg_cmd("service", "start", "--force")
    time.sleep(3)

    r = wg_cmd("service", "status")
    if r:
        for line in r.stdout.strip().split('\n')[:3]:
            print(f"  {line}")


# -- Inject chat and tasks -------------------------------------------------

def inject_chat_and_tasks():
    """Background: write chat history + create tasks so TUI auto-refreshes."""
    # Write chat history with the user message and coordinator response
    chat = [
        {
            "role": "user",
            "text": PROMPT,
            "timestamp": "2026-03-23T12:00:01+00:00",
            "edited": False,
        },
        {
            "role": "assistant",
            "text": CHAT_RESPONSE,
            "timestamp": "2026-03-23T12:00:08+00:00",
            "edited": False,
        },
    ]
    chat_file = os.path.join(DEMO_DIR, ".workgraph", "chat-history.json")
    with open(chat_file, "w") as f:
        json.dump(chat, f)

    # Create tasks one by one with small delays (simulates coordinator creating them)
    for title, tid, after in TASKS:
        cmd = ["add", title, "--id", tid,
               "-d", f"Implement: {title.lower()}"]
        if after:
            cmd.extend(["--after", after])
        wg_cmd(*cmd)
        time.sleep(0.3)  # Small delay so tasks appear incrementally in TUI

    print(f"  Injected chat history + {len(TASKS)} tasks")


# -- Recording -------------------------------------------------------------

def start_shell_and_recorder():
    """Start a tmux shell session (not TUI yet) and begin recording."""
    tmux("kill-session", "-t", SESSION)
    tmux(
        "new-session", "-d", "-s", SESSION,
        "-x", str(COLS), "-y", str(ROWS),
        f"cd {DEMO_DIR} && export PS1='\\[\\033[1;32m\\]$\\[\\033[0m\\] ' && exec bash --norc --noprofile",
    )
    tmux("resize-window", "-t", SESSION, "-x", str(COLS), "-y", str(ROWS))
    # Set PS1 inside the shell
    send_literal("export PS1='\\[\\033[1;32m\\]$ \\[\\033[0m\\]'\n")
    time.sleep(0.5)
    send_keys("C-l")  # Clear screen
    time.sleep(0.5)

    r = tmux("display-message", "-t", SESSION, "-p",
             "#{pane_width}x#{pane_height}")
    if r:
        print(f"  tmux pane: {r.stdout.strip()}")

    os.makedirs(os.path.dirname(CAST_FILE), exist_ok=True)
    recorder = subprocess.Popen([
        sys.executable,
        os.path.join(SCRIPT_DIR, "capture-tmux.py"),
        SESSION, CAST_FILE,
        "--cols", str(COLS), "--rows", str(ROWS), "--fps", str(FPS),
    ])
    time.sleep(1)
    print(f"  Recorder PID {recorder.pid}, {FPS}fps")
    return recorder


def launch_tui():
    """Launch the TUI inside the existing tmux session."""
    send_literal("wg tui --recording\n")
    time.sleep(4)

    screen = capture_pane()
    loaded = "Graph" in screen or "tasks" in screen.lower() or "LIVE" in screen
    print(f"  TUI loaded: {loaded}")


# -- Main recording flow --------------------------------------------------

def record():
    recorder = None

    try:
        # -- Phase 0: Setup --
        print("\n=== Phase 0: Setup ===")
        setup_demo()
        start_service()

        # -- Phase 1: Start shell + recorder --
        print("\n=== Phase 1: Start shell + recorder ===")
        recorder = start_shell_and_recorder()

        # -- Phase 1.5: CLI intro --
        print("\n=== Phase 1.5: CLI intro ===")
        time.sleep(1)

        # Show wg status
        type_naturally("wg status")
        send_keys("Enter")
        time.sleep(2.5)

        # Show wg add with a task
        type_naturally("wg add \"Parse input\" --id parse-input")
        send_keys("Enter")
        time.sleep(1.5)

        # Show wg add with dependency
        type_naturally("wg add \"Validate\" --after parse-input")
        send_keys("Enter")
        time.sleep(1.5)

        # Show wg viz
        type_naturally("wg viz")
        send_keys("Enter")
        time.sleep(3)

        print("  CLI intro complete")

        # Reinitialize WG state for a clean TUI demo
        subprocess.run(["rm", "-rf", os.path.join(DEMO_DIR, ".workgraph")])
        wg_cmd("init")
        wg_cmd("config", "--max-agents", "0")
        config_path = os.path.join(DEMO_DIR, ".workgraph", "config.toml")
        with open(config_path) as f:
            config = f.read()
        config = config.replace('coordinator_agent = true', 'coordinator_agent = false')
        config = config.replace('show_system_tasks = true', 'show_system_tasks = false')
        with open(config_path, "w") as f:
            f.write(config)
        start_service()

        # -- Phase 2: Launch TUI --
        print("\n=== Phase 2: Launch TUI ===")
        launch_tui()
        time.sleep(3)

        # -- Phase 3: Type prompt in chat --
        print("\n=== Phase 3: Type chat prompt ===")
        send_keys("c")  # Enter chat input mode
        time.sleep(1)

        type_naturally(PROMPT)
        time.sleep(0.5)
        send_keys("Enter")
        print(f"  Submitted: {PROMPT}")

        # -- Phase 4: Inject response + tasks --
        print("\n=== Phase 4: Inject chat response + tasks ===")
        time.sleep(2)  # Brief "thinking" pause before response appears
        inject_chat_and_tasks()
        time.sleep(3)  # Let TUI refresh and viewer absorb the new content

        # -- Phase 5: Navigate tasks --
        print("\n=== Phase 5: Navigate tasks ===")
        send_keys("Escape")  # Ensure normal mode
        time.sleep(0.5)

        # Navigate down through task list
        for i in range(4):
            send_keys("Down")
            time.sleep(0.7)
        time.sleep(1.5)

        # Navigate back up
        for i in range(2):
            send_keys("Up")
            time.sleep(0.5)
        time.sleep(1)

        # -- Phase 6: Inspector tabs --
        print("\n=== Phase 6: Inspector tabs ===")

        send_keys("1")  # Detail tab
        time.sleep(3)
        print("  Detail tab")

        send_keys("2")  # Log tab
        time.sleep(2)
        print("  Log tab")

        send_keys("0")  # Chat tab
        time.sleep(3)
        print("  Chat tab")

        # -- Phase 7: Task progression --
        print("\n=== Phase 7: Task progression ===")

        for batch, hold_time in PROGRESSION:
            # Claim batch (parallel execution)
            for tid in batch:
                wg_cmd("claim", tid)
                print(f"    {tid}: in-progress")

            # Navigate to show the in-progress tasks
            send_keys("Down")
            time.sleep(hold_time)

            # Complete batch
            for tid in batch:
                wg_cmd("done", tid)
                print(f"    {tid}: done")
            time.sleep(1)

        time.sleep(2)
        print("  All tasks completed")

        # -- Phase 8: Final navigation --
        print("\n=== Phase 8: Final navigation ===")

        # Go to top and navigate
        send_keys("Home")
        time.sleep(0.5)
        for i in range(3):
            send_keys("Down")
            time.sleep(0.7)

        # Show detail of a completed task
        send_keys("1")
        time.sleep(3)

        # Back to chat to show the conversation
        send_keys("0")
        time.sleep(2)

        # -- Phase 9: Exit --
        print("\n=== Phase 9: Exit ===")
        send_keys("q")
        time.sleep(2)

        recorder.terminate()
        recorder.wait()
        recorder = None

        if os.path.exists(CAST_FILE):
            with open(CAST_FILE) as f:
                lines = f.readlines()
            if len(lines) > 1:
                last = json.loads(lines[-1])
                print(f"\n=== Recording complete ===")
                print(f"File: {CAST_FILE}")
                print(f"Frames: {len(lines) - 1}")
                print(f"Duration: {last[0]:.1f}s")
                return True
        print("ERROR: No recording produced!")
        return False

    finally:
        if recorder:
            recorder.terminate()
            recorder.wait()
        tmux("kill-session", "-t", SESSION)
        wg_cmd("service", "stop")
        print(f"Demo dir: {DEMO_DIR}")


if __name__ == "__main__":
    success = record()
    sys.exit(0 if success else 1)
