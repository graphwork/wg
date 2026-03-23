#!/usr/bin/env python3
"""Compress hero-v3-raw.cast into a snappy time-lapse.

Strategy:
- Typing/interaction (small gaps <0.5s): keep at ~1x
- Activity (TUI updating, gaps 0.5-2s): play at ~2x
- Waiting (gaps >2s): compress to 0.3-0.5s
- Target: 30-60 seconds total
"""

import json
import sys

RAW_PATH = "screencast/recordings/hero-v3-raw.cast"
COMPRESSED_PATH = "screencast/recordings/hero-v3-compressed.cast"
TIMEMAP_PATH = "screencast/recordings/hero-v3-timemap.json"

# Compression parameters
MAX_WAIT_GAP = 0.4       # Gaps >2s compressed to this
MEDIUM_GAP_CAP = 0.6     # Gaps 1-2s compressed to this
ACTIVITY_SPEED = 2.0     # Speed multiplier for gaps 0.3-1s
INTERACTION_SPEED = 1.0  # Speed multiplier for gaps <0.3s (typing)


def load_cast(path):
    with open(path, "r") as f:
        lines = f.readlines()
    header = json.loads(lines[0])
    frames = []
    for line in lines[1:]:
        frame = json.loads(line)
        frames.append(frame)
    return header, frames, lines


def classify_and_compress(frames):
    """Compute compressed timestamps for each frame."""
    if not frames:
        return [], []

    compressed_time = frames[0][0]  # Start at same offset as original
    timemap = [{"compressed_s": round(compressed_time, 6), "real_s": round(frames[0][0], 6)}]
    new_timestamps = [compressed_time]

    for i in range(1, len(frames)):
        real_gap = frames[i][0] - frames[i - 1][0]

        if real_gap > 2.0:
            # Long wait — compress heavily
            compressed_gap = MAX_WAIT_GAP
        elif real_gap > 1.0:
            # Medium wait — cap it
            compressed_gap = MEDIUM_GAP_CAP
        elif real_gap > 0.3:
            # Activity — speed up
            compressed_gap = real_gap / ACTIVITY_SPEED
        else:
            # Interaction/typing — keep natural
            compressed_gap = real_gap / INTERACTION_SPEED

        compressed_time += compressed_gap
        new_timestamps.append(compressed_time)
        timemap.append({
            "compressed_s": round(compressed_time, 6),
            "real_s": round(frames[i][0], 6),
        })

    return new_timestamps, timemap


def write_compressed(header, frames, new_timestamps, raw_lines, out_path):
    """Write compressed cast file preserving exact frame content and CR+LF."""
    with open(out_path, "w", newline="") as f:
        # Write header line exactly as-is (preserve any trailing newline style)
        header_line = raw_lines[0]
        f.write(header_line)
        if not header_line.endswith("\n"):
            f.write("\n")

        for i, frame in enumerate(frames):
            # Replace only the timestamp, keep original content byte-for-byte
            new_frame = [round(new_timestamps[i], 6), frame[1], frame[2]]
            line = json.dumps(new_frame, ensure_ascii=False)
            f.write(line + "\n")


def main():
    print(f"Loading {RAW_PATH}...")
    header, frames, raw_lines = load_cast(RAW_PATH)

    print(f"  Header: {header['width']}x{header['height']}, {len(frames)} frames")
    raw_duration = frames[-1][0] - frames[0][0]
    print(f"  Raw duration: {raw_duration:.1f}s ({raw_duration/60:.1f} min)")

    print("Compressing...")
    new_timestamps, timemap = classify_and_compress(frames)

    compressed_duration = new_timestamps[-1] - new_timestamps[0]
    ratio = raw_duration / compressed_duration
    print(f"  Compressed duration: {compressed_duration:.1f}s")
    print(f"  Compression ratio: {ratio:.1f}x")

    if compressed_duration < 30 or compressed_duration > 60:
        print(f"  WARNING: Duration {compressed_duration:.1f}s outside target 30-60s range")

    # Verify frame content is identical
    for i, frame in enumerate(frames):
        assert frame[1] == frames[i][1], f"Frame {i} type changed"
        assert frame[2] == frames[i][2], f"Frame {i} content changed"

    print(f"Writing {COMPRESSED_PATH}...")
    write_compressed(header, frames, new_timestamps, raw_lines, COMPRESSED_PATH)

    print(f"Writing {TIMEMAP_PATH}...")
    with open(TIMEMAP_PATH, "w") as f:
        json.dump(timemap, f, indent=2)

    # Validation
    print("\nValidation:")
    # Check dimensions preserved
    with open(COMPRESSED_PATH) as f:
        comp_header = json.loads(f.readline())
    assert comp_header["width"] == 65, f"Width changed: {comp_header['width']}"
    assert comp_header["height"] == 38, f"Height changed: {comp_header['height']}"
    print(f"  ✓ Dimensions preserved: {comp_header['width']}x{comp_header['height']}")

    # Check no gaps > 1s with no content changes
    with open(COMPRESSED_PATH) as f:
        comp_lines = f.readlines()
    comp_frames = [json.loads(line) for line in comp_lines[1:]]
    max_gap = 0
    for i in range(1, len(comp_frames)):
        gap = comp_frames[i][0] - comp_frames[i - 1][0]
        if gap > max_gap:
            max_gap = gap
    print(f"  ✓ Max gap in compressed: {max_gap:.2f}s (target: <1s)")

    # Check frame count matches
    assert len(comp_frames) == len(frames), "Frame count mismatch"
    print(f"  ✓ Frame count preserved: {len(comp_frames)}")

    # Check content identical
    for i in range(len(frames)):
        assert comp_frames[i][1] == frames[i][1], f"Frame {i} type differs"
        assert comp_frames[i][2] == frames[i][2], f"Frame {i} content differs"
    print(f"  ✓ All frame content identical")

    # Check timemap
    assert len(timemap) == len(frames), "Timemap size mismatch"
    print(f"  ✓ Timemap has {len(timemap)} entries")

    # Check CR+LF preserved in content
    crlf_count = sum(1 for f in comp_frames if "\r\n" in f[2])
    print(f"  ✓ CR+LF frames: {crlf_count}/{len(comp_frames)}")

    print(f"\nDone! {raw_duration:.1f}s → {compressed_duration:.1f}s ({ratio:.1f}x compression)")


if __name__ == "__main__":
    main()
