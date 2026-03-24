#!/usr/bin/env python3
"""Compress raw TUI interaction recordings into short, focused clips.

Each recording is compressed to its target duration (5-15s) by:
- Keeping keystroke timing natural (typing/navigation at real speed)
- Crushing idle pauses and TUI render waits
- Capping long gaps to maintain flow

Input:  screencast/recordings/tui-{name}-raw.cast
Output: website/assets/casts/tui-{name}.cast
"""

import json
import os
import re
import sys

RECORDINGS_DIR = "screencast/recordings"
OUTPUT_DIR = "website/assets/casts"

# Per-recording compression config:
#   (input_name, output_name, target_duration,
#    interaction_speed, activity_speed, short_wait_cap, long_wait_cap)
RECORDINGS = [
    ("tui-graph-nav-raw", "tui-graph-nav", 12,
     1.0, 2.0, 0.20, 0.12),
    ("tui-inspector-raw", "tui-inspector", 13,
     1.0, 2.0, 0.25, 0.15),
    ("tui-agent-monitor-raw", "tui-agent-monitor", 13,
     1.0, 2.0, 0.20, 0.12),
    ("tui-key-workflows-raw", "tui-key-workflows", 14,
     1.0, 2.5, 0.15, 0.08),
]


def load_cast(path):
    with open(path, "r") as f:
        lines = f.readlines()
    header = json.loads(lines[0])
    frames = [json.loads(line) for line in lines[1:]]
    return header, frames, lines


def compress(frames, interaction_speed, activity_speed, short_wait_cap, long_wait_cap):
    """Compress frame timestamps."""
    if not frames:
        return [], []

    compressed_time = 0.0
    new_timestamps = [0.0]

    for i in range(1, len(frames)):
        real_gap = frames[i][0] - frames[i - 1][0]

        if real_gap > 3.0:
            compressed_gap = long_wait_cap
        elif real_gap > 1.0:
            compressed_gap = short_wait_cap
        elif real_gap > 0.3:
            compressed_gap = real_gap / activity_speed
        else:
            compressed_gap = real_gap / interaction_speed

        compressed_time += compressed_gap
        new_timestamps.append(compressed_time)

    return new_timestamps


def write_compressed(header, frames, new_timestamps, raw_lines, out_path):
    """Write compressed cast file."""
    os.makedirs(os.path.dirname(out_path), exist_ok=True)

    with open(out_path, "w", newline="") as f:
        # Preserve original header
        header_line = raw_lines[0]
        f.write(header_line)
        if not header_line.endswith("\n"):
            f.write("\n")

        for i, frame in enumerate(frames):
            new_frame = [round(new_timestamps[i], 6), frame[1], frame[2]]
            line = json.dumps(new_frame, ensure_ascii=False)
            f.write(line + "\n")


def process_recording(input_name, output_name, target_dur,
                      interaction_speed, activity_speed,
                      short_wait_cap, long_wait_cap):
    """Process one recording."""
    input_path = os.path.join(RECORDINGS_DIR, f"{input_name}.cast")
    output_path = os.path.join(OUTPUT_DIR, f"{output_name}.cast")

    if not os.path.exists(input_path):
        print(f"  SKIP: {input_path} not found")
        return False

    print(f"\n  Processing {input_name}...")
    header, frames, raw_lines = load_cast(input_path)
    raw_duration = frames[-1][0] - frames[0][0] if frames else 0
    print(f"    Raw: {raw_duration:.1f}s, {len(frames)} frames")

    new_timestamps = compress(
        frames, interaction_speed, activity_speed,
        short_wait_cap, long_wait_cap,
    )

    compressed_duration = new_timestamps[-1] if new_timestamps else 0
    ratio = raw_duration / compressed_duration if compressed_duration > 0 else 0
    print(f"    Compressed: {compressed_duration:.1f}s ({ratio:.1f}x)")

    if compressed_duration < 3 or compressed_duration > 25:
        print(f"    WARNING: Duration {compressed_duration:.1f}s outside 3-25s range")

    write_compressed(header, frames, new_timestamps, raw_lines, output_path)
    print(f"    Output: {output_path}")

    # Validate
    with open(output_path) as f:
        comp_lines = f.readlines()
    comp_frames = [json.loads(line) for line in comp_lines[1:]]
    assert len(comp_frames) == len(frames), "Frame count mismatch"

    for i in range(len(frames)):
        assert comp_frames[i][2] == frames[i][2], f"Frame {i} content differs"
    print(f"    Validated: {len(comp_frames)} frames, content identical")

    return True


def main():
    print("=== Compressing TUI Interaction Recordings ===")

    results = {}
    for input_name, output_name, target_dur, *params in RECORDINGS:
        ok = process_recording(input_name, output_name, target_dur, *params)
        results[output_name] = ok

    print(f"\n{'=' * 50}")
    for name, ok in results.items():
        marker = "+" if ok else "-"
        print(f"  {marker} {name}")

    skipped = sum(1 for ok in results.values() if not ok)
    if skipped:
        print(f"\n  {skipped} recording(s) skipped (run record-tui-interactions.py first)")

    print("Done!")


if __name__ == "__main__":
    main()
