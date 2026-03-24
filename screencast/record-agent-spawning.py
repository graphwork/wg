#!/usr/bin/env python3
"""Generate a synthetic screencast showing agent spawning and parallel execution.

Produces a 15-second .cast file with frames showing:
1. Task list with open tasks ready to go
2. Service start → agents begin spawning
3. Multiple agents running in parallel, tasks transitioning to in-progress
4. Worktree isolation visible in agent details
5. Tasks completing as agents finish work

Output: screencast/recordings/agent-spawning-raw.cast
"""

import json
import os
import re
import time

SCRIPT_DIR = os.path.dirname(os.path.abspath(__file__))
CAST_FILE = os.path.join(SCRIPT_DIR, "recordings", "agent-spawning-raw.cast")

# Terminal dimensions (match other screencasts)
COLS = 80
ROWS = 24

# ANSI color codes
RESET = "\033[0m"
BOLD = "\033[1m"
DIM = "\033[2m"
GREEN = "\033[32m"
YELLOW = "\033[33m"
CYAN = "\033[36m"
MAGENTA = "\033[35m"
WHITE = "\033[37m"
BOLD_GREEN = "\033[1;32m"
BOLD_YELLOW = "\033[1;33m"
BOLD_CYAN = "\033[1;36m"
BOLD_WHITE = "\033[1;37m"
BOLD_MAGENTA = "\033[1;35m"
BG_BLACK = "\033[40m"


def make_frame(lines, cols=COLS, rows=ROWS):
    """Build a terminal frame from lines of text.

    Pads/truncates to exactly cols x rows. Converts to CR+LF.
    Prepends clear-screen + cursor-home escape.
    """
    # Pad to rows
    while len(lines) < rows:
        lines.append("")

    # Truncate to rows
    lines = lines[:rows]

    # Join with CR+LF (terminal line endings)
    content = "\r\n".join(lines)

    # Clear screen + cursor home
    return "\033[H\033[2J" + content


def build_cast():
    """Build the complete .cast file content."""
    header = {
        "version": 2,
        "width": COLS,
        "height": ROWS,
        "timestamp": int(time.time()),
        "env": {"TERM": "xterm-256color", "SHELL": "/bin/bash"},
        "idle_time_limit": 2.0,
    }

    events = []

    def add_frame(t, lines):
        """Add a frame at time t with the given lines."""
        frame = make_frame(list(lines))  # copy the list
        events.append([round(t, 3), "o", frame])

    # ── Prompt helper ──
    prompt = f"{BOLD_GREEN}${RESET} "

    # ═══════════════════════════════════════════════════════════
    # Frame 0 (t=0.0): Shell prompt, about to type
    # ═══════════════════════════════════════════════════════════
    add_frame(0.0, [prompt])

    # ═══════════════════════════════════════════════════════════
    # Frame 1 (t=0.3): Typing "wg list"
    # ═══════════════════════════════════════════════════════════
    add_frame(0.3, [prompt + "w"])
    add_frame(0.42, [prompt + "wg"])
    add_frame(0.55, [prompt + "wg "])
    add_frame(0.65, [prompt + "wg l"])
    add_frame(0.75, [prompt + "wg li"])
    add_frame(0.85, [prompt + "wg lis"])
    add_frame(0.95, [prompt + "wg list"])

    # ═══════════════════════════════════════════════════════════
    # Frame 2 (t=1.1): wg list output — tasks all open
    # ═══════════════════════════════════════════════════════════
    list_open = [
        prompt + "wg list",
        f"{BOLD_WHITE}8 tasks{RESET}",
        "",
        f"[ ] scrape-headlines       {DIM}Scrape top news headlines{RESET}",
        f"[ ] analyze-mood           {DIM}Sentiment analysis on headlines{RESET}        {DIM}← scrape-headlines{RESET}",
        f"[ ] count-syllables        {DIM}Build syllable-counting engine{RESET}",
        f"[ ] build-pun-db           {DIM}Collect wordplay database{RESET}",
        f"[ ] wire-haiku-engine      {DIM}Core haiku generator{RESET}                  {DIM}← scrape-headlines, count-syllables{RESET}",
        f"[ ] draft-haikus           {DIM}Generate haiku per headline{RESET}            {DIM}← wire-haiku-engine, analyze-mood{RESET}",
        f"[ ] review-quality         {DIM}Quality gate on generated haiku{RESET}        {DIM}← draft-haikus{RESET}",
        f"[ ] publish-api            {DIM}REST API serving approved haiku{RESET}        {DIM}← review-quality, build-pun-db{RESET}",
        "",
        prompt,
    ]
    add_frame(1.1, list_open)

    # ═══════════════════════════════════════════════════════════
    # Frame 3 (t=2.5): Typing "wg service start"
    # ═══════════════════════════════════════════════════════════
    base = list_open[:-1]  # Remove trailing prompt
    add_frame(2.5, base + [prompt + "w"])
    add_frame(2.6, base + [prompt + "wg"])
    add_frame(2.7, base + [prompt + "wg s"])
    add_frame(2.8, base + [prompt + "wg se"])
    add_frame(2.9, base + [prompt + "wg ser"])
    add_frame(3.0, base + [prompt + "wg serv"])
    add_frame(3.1, base + [prompt + "wg servi"])
    add_frame(3.15, base + [prompt + "wg servic"])
    add_frame(3.2, base + [prompt + "wg service"])
    add_frame(3.3, base + [prompt + "wg service "])
    add_frame(3.4, base + [prompt + "wg service s"])
    add_frame(3.5, base + [prompt + "wg service st"])
    add_frame(3.6, base + [prompt + "wg service sta"])
    add_frame(3.65, base + [prompt + "wg service star"])
    add_frame(3.7, base + [prompt + "wg service start"])

    # ═══════════════════════════════════════════════════════════
    # Frame 4 (t=3.9): Service start output
    # ═══════════════════════════════════════════════════════════
    service_out = base + [
        prompt + "wg service start",
        f"{BOLD_GREEN}Service started{RESET} (max_agents: 4, model: sonnet)",
        f"{DIM}Coordinator spawning...{RESET}",
        prompt,
    ]
    add_frame(3.9, service_out)

    # ═══════════════════════════════════════════════════════════
    # Frame 5 (t=4.8): Typing "wg agents --alive"
    # ═══════════════════════════════════════════════════════════
    svc_base = service_out[:-1]
    add_frame(4.8, svc_base + [prompt + "wg agents --alive"])

    # ═══════════════════════════════════════════════════════════
    # Frame 6 (t=5.0): First agent spawned — 1 task claimed
    # ═══════════════════════════════════════════════════════════
    agents_1 = svc_base + [
        prompt + "wg agents --alive",
        f"{BOLD_WHITE}ID           TASK                   EXECUTOR   PID     UPTIME  STATUS{RESET}",
        f"agent-1001   scrape-headlines       claude     48201       3s  {BOLD_GREEN}working{RESET}",
        "",
        f"{DIM}1 agent(s){RESET}",
        "",
        prompt,
    ]
    add_frame(5.0, agents_1)

    # ═══════════════════════════════════════════════════════════
    # Frame 7 (t=6.3): Typing "wg agents --alive" again
    # ═══════════════════════════════════════════════════════════
    a1_base = agents_1[:-1]
    add_frame(6.3, a1_base + [prompt + "wg agents --alive"])

    # ═══════════════════════════════════════════════════════════
    # Frame 8 (t=6.5): 3 agents now — parallel fan-out
    # ═══════════════════════════════════════════════════════════
    agents_3 = a1_base + [
        prompt + "wg agents --alive",
        f"{BOLD_WHITE}ID           TASK                   EXECUTOR   PID     UPTIME  STATUS{RESET}",
        f"agent-1001   scrape-headlines       claude     48201      12s  {BOLD_GREEN}working{RESET}",
        f"agent-1002   count-syllables        claude     48215       8s  {BOLD_GREEN}working{RESET}",
        f"agent-1003   build-pun-db           claude     48230       5s  {BOLD_GREEN}working{RESET}",
        "",
        f"{DIM}3 agent(s){RESET}",
        "",
        prompt,
    ]
    add_frame(6.5, agents_3)

    # ═══════════════════════════════════════════════════════════
    # Frame 9 (t=8.0): Typing "wg list" to show transitions
    # ═══════════════════════════════════════════════════════════
    a3_base = agents_3[:-1]
    add_frame(8.0, a3_base + [prompt + "wg list"])

    # ═══════════════════════════════════════════════════════════
    # Frame 10 (t=8.2): wg list showing mixed states
    # ═══════════════════════════════════════════════════════════
    list_mixed = [
        prompt + "wg list",
        f"{BOLD_WHITE}8 tasks (3 active, 5 open){RESET}",
        "",
        f"[{BOLD_YELLOW}\u25cf{RESET}] scrape-headlines       {BOLD_YELLOW}in-progress{RESET}  {DIM}agent-1001  12s{RESET}",
        f"[ ] analyze-mood           {DIM}blocked{RESET}                       {DIM}\u2190 scrape-headlines{RESET}",
        f"[{BOLD_YELLOW}\u25cf{RESET}] count-syllables        {BOLD_YELLOW}in-progress{RESET}  {DIM}agent-1002   8s{RESET}",
        f"[{BOLD_YELLOW}\u25cf{RESET}] build-pun-db           {BOLD_YELLOW}in-progress{RESET}  {DIM}agent-1003   5s{RESET}",
        f"[ ] wire-haiku-engine      {DIM}blocked{RESET}                       {DIM}\u2190 scrape-headlines, count-syllables{RESET}",
        f"[ ] draft-haikus           {DIM}blocked{RESET}                       {DIM}\u2190 wire-haiku-engine, analyze-mood{RESET}",
        f"[ ] review-quality         {DIM}blocked{RESET}                       {DIM}\u2190 draft-haikus{RESET}",
        f"[ ] publish-api            {DIM}blocked{RESET}                       {DIM}\u2190 review-quality, build-pun-db{RESET}",
        "",
        prompt,
    ]
    add_frame(8.2, list_mixed)

    # ═══════════════════════════════════════════════════════════
    # Frame 11 (t=10.0): Typing "wg agents --alive" for worktree view
    # ═══════════════════════════════════════════════════════════
    lm_base = list_mixed[:-1]
    add_frame(10.0, lm_base + [prompt + "wg agents --alive"])

    # ═══════════════════════════════════════════════════════════
    # Frame 12 (t=10.2): 4 agents — max parallel, tasks advancing
    #   scrape-headlines done, analyze-mood + wire-haiku now claimed
    # ═══════════════════════════════════════════════════════════
    agents_4 = [
        prompt + "wg agents --alive",
        f"{BOLD_WHITE}ID           TASK                   EXECUTOR   PID     UPTIME  STATUS{RESET}",
        f"agent-1002   count-syllables        claude     48215      30s  {BOLD_GREEN}working{RESET}",
        f"agent-1003   build-pun-db           claude     48230      27s  {BOLD_GREEN}working{RESET}",
        f"agent-1004   analyze-mood           claude     48301       4s  {BOLD_GREEN}working{RESET}",
        f"agent-1005   wire-haiku-engine      claude     48315       2s  {BOLD_GREEN}working{RESET}",
        "",
        f"{DIM}4 agent(s)  {BOLD_WHITE}scrape-headlines{RESET}{DIM} done \u2192 2 new agents spawned{RESET}",
        "",
        f"{DIM}Worktrees:{RESET}",
        f"  {DIM}agent-1002:{RESET} .worktrees/count-syllables-1002/",
        f"  {DIM}agent-1003:{RESET} .worktrees/build-pun-db-1003/",
        f"  {DIM}agent-1004:{RESET} .worktrees/analyze-mood-1004/",
        f"  {DIM}agent-1005:{RESET} .worktrees/wire-haiku-engine-1005/",
        "",
        prompt,
    ]
    add_frame(10.2, agents_4)

    # ═══════════════════════════════════════════════════════════
    # Frame 13 (t=12.5): Final "wg list" showing completions
    # ═══════════════════════════════════════════════════════════
    a4_base = agents_4[:-1]
    add_frame(12.5, a4_base + [prompt + "wg list"])

    list_final = [
        prompt + "wg list",
        f"{BOLD_WHITE}8 tasks (1 done, 4 active, 3 waiting){RESET}",
        "",
        f"[{BOLD_GREEN}\u2713{RESET}] scrape-headlines       {BOLD_GREEN}done{RESET}           {DIM}14s{RESET}",
        f"[{BOLD_YELLOW}\u25cf{RESET}] analyze-mood           {BOLD_YELLOW}in-progress{RESET}  {DIM}agent-1004   4s{RESET}",
        f"[{BOLD_YELLOW}\u25cf{RESET}] count-syllables        {BOLD_YELLOW}in-progress{RESET}  {DIM}agent-1002  30s{RESET}",
        f"[{BOLD_YELLOW}\u25cf{RESET}] build-pun-db           {BOLD_YELLOW}in-progress{RESET}  {DIM}agent-1003  27s{RESET}",
        f"[{BOLD_YELLOW}\u25cf{RESET}] wire-haiku-engine      {BOLD_YELLOW}in-progress{RESET}  {DIM}agent-1005   2s{RESET}",
        f"[ ] draft-haikus           {DIM}blocked{RESET}                       {DIM}\u2190 wire-haiku-engine, analyze-mood{RESET}",
        f"[ ] review-quality         {DIM}blocked{RESET}                       {DIM}\u2190 draft-haikus{RESET}",
        f"[ ] publish-api            {DIM}blocked{RESET}                       {DIM}\u2190 review-quality, build-pun-db{RESET}",
        "",
        f"{DIM}Agents auto-spawn on ready tasks \u2022 each in its own worktree{RESET}",
        "",
        prompt,
    ]
    add_frame(12.7, list_final)

    # ═══════════════════════════════════════════════════════════
    # Frame 14 (t=15.0): Hold final frame
    # ═══════════════════════════════════════════════════════════
    add_frame(15.0, list_final)

    return header, events


def write_cast(header, events, path):
    """Write header + events to a .cast file."""
    os.makedirs(os.path.dirname(path), exist_ok=True)
    with open(path, "w") as f:
        f.write(json.dumps(header) + "\n")
        for event in events:
            f.write(json.dumps(event) + "\n")


def verify_cast(path):
    """Verify the .cast file is valid asciinema v2."""
    with open(path) as f:
        lines = f.readlines()

    header = json.loads(lines[0])
    assert header["version"] == 2, f"Bad version: {header['version']}"
    assert header["width"] == COLS
    assert header["height"] == ROWS

    frame_count = len(lines) - 1
    last = json.loads(lines[-1])
    duration = last[0]

    # Check all frames are valid JSON with [time, type, data]
    prev_t = -1
    for i, line in enumerate(lines[1:], 1):
        event = json.loads(line)
        assert len(event) == 3, f"Frame {i}: bad length {len(event)}"
        assert isinstance(event[0], (int, float)), f"Frame {i}: bad timestamp"
        assert event[0] >= prev_t, f"Frame {i}: non-monotonic timestamp"
        assert event[1] == "o", f"Frame {i}: bad type '{event[1]}'"
        prev_t = event[0]

    # Check bare LF in frame data
    bare_lf = 0
    for line in lines[1:]:
        event = json.loads(line)
        data = event[2]
        bare_lf += len(re.findall(r'(?<!\r)\n', data))

    print(f"  Frames: {frame_count}")
    print(f"  Duration: {duration:.1f}s")
    print(f"  Dimensions: {header['width']}x{header['height']}")
    print(f"  Bare LF: {bare_lf}")

    ok = bare_lf == 0 and 10 <= duration <= 20 and frame_count > 5
    if ok:
        print("  ALL CHECKS PASSED")
    else:
        if bare_lf > 0:
            print("  WARNING: bare LF found")
        if not (10 <= duration <= 20):
            print(f"  WARNING: duration {duration:.1f}s not in 10-20s range")

    return ok


if __name__ == "__main__":
    print(f"Generating agent spawning screencast...")
    print(f"Output: {CAST_FILE}")

    header, events = build_cast()
    write_cast(header, events, CAST_FILE)

    size = os.path.getsize(CAST_FILE)
    print(f"Size: {size} bytes")
    print()
    print("Verifying...")
    ok = verify_cast(CAST_FILE)

    if ok:
        print(f"\nDone! Preview: asciinema play {CAST_FILE}")
    else:
        print("\nWARNING: verification issues detected")
