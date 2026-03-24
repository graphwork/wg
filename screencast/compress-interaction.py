#!/usr/bin/env python3
"""Compress interaction-raw.cast into a 45-60s time-lapse with scene markers.

Scene-aware compression tuned for the interaction screencast:
- Scene 1 (Launch): moderate — typing visible
- Scene 2 (Chat): typing real-speed, coordinator think-time crushed
- Scene 3 (Agents): heavy on wait, keep status transitions visible
- Scene 4 (Detail View): moderate — live output is the payoff
- Scene 5 (Round 2): same as Scene 2
- Scene 6 (Survey + Exit): moderate, navigation real-speed
"""

import json
import re
import sys

RAW_PATH = "screencast/recordings/interaction-raw.cast"
COMPRESSED_PATH = "screencast/recordings/interaction-compressed.cast"
TIMEMAP_PATH = "screencast/recordings/interaction-timemap.json"

# Scene transition pause
SCENE_PAUSE = 0.5

# Per-scene compression: (interaction_speed, activity_speed, short_wait_cap, long_wait_cap)
#   interaction_speed: multiplier for gaps <0.3s (typing/keystrokes)
#   activity_speed: multiplier for gaps 0.3-1s (TUI updating)
#   short_wait_cap: cap for gaps 1-3s
#   long_wait_cap: cap for gaps >3s
SCENE_PARAMS = {
    # Scene 1 (~5s target): typing + TUI launch
    "launch":      (1.0, 2.0, 0.30, 0.25),
    # Scene 2 (~8s target): typing real-speed, coordinator wait crushed
    "chat":        (1.0, 1.5, 0.20, 0.15),
    # Scene 3 (~10s target): agent spawn waits crushed, transitions visible
    "agents":      (1.5, 3.0, 0.15, 0.10),
    # Scene 4 (~15s target): moderate — live output is the key moment
    "detail":      (1.0, 2.0, 0.30, 0.25),
    # Scene 5 (~7s target): typing real-speed, coordinator wait crushed
    "round2":      (1.0, 1.5, 0.20, 0.15),
    # Scene 6 (~6s target): navigation + exit
    "survey":      (1.5, 2.5, 0.20, 0.15),
    # Exit
    "exit":        (1.0, 2.0, 0.30, 0.20),
}

DEFAULT_PARAMS = (1.5, 3.0, 0.20, 0.15)


def load_cast(path):
    with open(path, "r") as f:
        lines = f.readlines()
    header = json.loads(lines[0])
    frames = [json.loads(line) for line in lines[1:]]
    return header, frames, lines


def detect_scenes(frames):
    """Detect scene boundaries from content.

    Returns list of (scene_name, start_frame_idx).
    """
    scenes = [("launch", 0)]
    tui_entered = False
    chat_started = False
    agents_started = False
    detail_started = False
    round2_started = False
    survey_started = False

    for i, frame in enumerate(frames):
        clean = re.sub(r'\x1b\[[^a-zA-Z]*[a-zA-Z]', '', frame[2])

        # TUI loaded — transition to chat scene
        if not tui_entered and ("Chat" in clean or "LIVE" in clean):
            # Look for task count in status bar
            if re.search(r'\d+ tasks', clean):
                tui_entered = True
                scenes.append(("chat", i))
                continue

        if not tui_entered:
            continue

        # Chat input submitted (detect coordinator response or task count jump)
        m = re.search(r'(\d+) tasks \((\d+) done', clean)
        if m:
            task_count = int(m.group(1))

            # Agent spawn phase: tasks > 1 and some activity
            if not agents_started and task_count >= 5:
                agents_started = True
                scenes.append(("agents", i))

            # Detail view phase: detect tab switch to Detail/Log/Firehose
            if not detail_started and agents_started:
                # Look for Detail tab content indicators
                if ("Detail" in clean and "Status" in clean) or \
                   ("Log" in clean and re.search(r'\d{4}-\d{2}-\d{2}', clean)) or \
                   "Fire" in clean:
                    detail_started = True
                    scenes.append(("detail", i))

            # Round 2: task count jumps again (roast tasks added)
            if not round2_started and task_count >= 9:
                # Check if roast-related content visible
                if "snark" in clean.lower() or "roast" in clean.lower() or task_count >= 10:
                    round2_started = True
                    # Look back to find typing start
                    typing_start = i
                    for j in range(max(0, i - 30), i):
                        gap = frames[j][0] - frames[j - 1][0] if j > 0 else 0
                        if gap < 0.25:
                            typing_start = j
                            break
                    scenes.append(("round2", typing_start))

        # Survey/exit: after round2, navigating back up
        if round2_started and not survey_started:
            # Detect navigation back up through the graph
            if i > 0 and (frames[i][0] - frames[i-1][0]) < 0.5:
                # Multiple fast frames = navigation
                survey_started = True
                scenes.append(("survey", i))

    # If we didn't detect all scenes, add reasonable defaults
    if not agents_started:
        # Place agents scene at 1/4 through
        scenes.append(("agents", len(frames) // 4))
    if not detail_started:
        scenes.append(("detail", len(frames) // 3))
    if not round2_started:
        scenes.append(("round2", 2 * len(frames) // 3))
    if not survey_started:
        scenes.append(("survey", 5 * len(frames) // 6))

    # Sort by frame index
    scenes.sort(key=lambda x: x[1])
    return scenes


def get_scene_at(frame_idx, scenes):
    """Return the scene name for a given frame index."""
    current = "launch"
    for name, start_idx in scenes:
        if frame_idx >= start_idx:
            current = name
        else:
            break
    return current


def classify_and_compress(frames, scenes):
    """Compute compressed timestamps with scene-aware compression."""
    if not frames:
        return [], [], {}

    scene_transitions = {idx for _, idx in scenes}

    compressed_time = 0.0
    timemap = [{"compressed_s": 0.0, "real_s": round(frames[0][0], 6)}]
    new_timestamps = [0.0]

    for i in range(1, len(frames)):
        real_gap = frames[i][0] - frames[i - 1][0]
        current_scene = get_scene_at(i, scenes)

        # Scene transition pause
        if i in scene_transitions:
            compressed_time += SCENE_PAUSE

        interaction_speed, activity_speed, short_wait_cap, long_wait_cap = \
            SCENE_PARAMS.get(current_scene, DEFAULT_PARAMS)

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
        timemap.append({
            "compressed_s": round(compressed_time, 6),
            "real_s": round(frames[i][0], 6),
        })

    # Per-scene durations
    scene_list = [(name, idx) for name, idx in scenes]
    scene_durations = {}
    for si in range(len(scene_list)):
        name, start_idx = scene_list[si]
        end_idx = scene_list[si + 1][1] if si + 1 < len(scene_list) else len(frames) - 1
        real_dur = frames[end_idx][0] - frames[start_idx][0]
        comp_dur = new_timestamps[end_idx] - new_timestamps[start_idx]
        scene_durations[name] = (real_dur, comp_dur)

    return new_timestamps, timemap, scene_durations


def write_compressed(header, frames, new_timestamps, raw_lines, out_path):
    """Write compressed cast file preserving exact frame content."""
    with open(out_path, "w", newline="") as f:
        header_line = raw_lines[0]
        f.write(header_line)
        if not header_line.endswith("\n"):
            f.write("\n")

        for i, frame in enumerate(frames):
            new_frame = [round(new_timestamps[i], 6), frame[1], frame[2]]
            line = json.dumps(new_frame, ensure_ascii=False)
            f.write(line + "\n")


def main():
    print(f"Loading {RAW_PATH}...")
    header, frames, raw_lines = load_cast(RAW_PATH)

    print(f"  Header: {header['width']}x{header['height']}, {len(frames)} frames")
    raw_duration = frames[-1][0] - frames[0][0]
    print(f"  Raw duration: {raw_duration:.1f}s ({raw_duration/60:.1f} min)")

    print("\nDetecting scenes...")
    scenes = detect_scenes(frames)
    for name, idx in scenes:
        print(f"  {name}: frame {idx}, t={frames[idx][0]:.1f}s")

    print("\nCompressing...")
    new_timestamps, timemap, scene_durations = classify_and_compress(frames, scenes)

    compressed_duration = new_timestamps[-1] - new_timestamps[0]
    ratio = raw_duration / compressed_duration if compressed_duration > 0 else 0
    print(f"  Compressed duration: {compressed_duration:.1f}s")
    print(f"  Compression ratio: {ratio:.1f}x")

    print("\nPer-scene durations:")
    for name, (real_dur, comp_dur) in scene_durations.items():
        scene_ratio = real_dur / comp_dur if comp_dur > 0 else float("inf")
        print(f"  {name:15s}: {real_dur:7.1f}s -> {comp_dur:5.1f}s ({scene_ratio:.1f}x)")

    if compressed_duration < 45 or compressed_duration > 75:
        print(f"\n  WARNING: Duration {compressed_duration:.1f}s outside target 45-60s range")

    print(f"\nWriting {COMPRESSED_PATH}...")
    write_compressed(header, frames, new_timestamps, raw_lines, COMPRESSED_PATH)

    print(f"Writing {TIMEMAP_PATH}...")
    with open(TIMEMAP_PATH, "w") as f:
        json.dump(timemap, f, indent=2)

    # Validation
    print("\nValidation:")

    with open(COMPRESSED_PATH) as f:
        comp_header = json.loads(f.readline())
    assert comp_header["width"] == 65, f"Width changed: {comp_header['width']}"
    assert comp_header["height"] == 38, f"Height changed: {comp_header['height']}"
    print(f"  Dimensions preserved: {comp_header['width']}x{comp_header['height']}")

    with open(COMPRESSED_PATH) as f:
        comp_lines = f.readlines()
    comp_frames = [json.loads(line) for line in comp_lines[1:]]

    max_gap = 0
    for i in range(1, len(comp_frames)):
        gap = comp_frames[i][0] - comp_frames[i - 1][0]
        if gap > max_gap:
            max_gap = gap
    print(f"  Max gap in compressed: {max_gap:.2f}s")

    assert len(comp_frames) == len(frames), "Frame count mismatch"
    print(f"  Frame count preserved: {len(comp_frames)}")

    for i in range(len(frames)):
        assert comp_frames[i][1] == frames[i][1], f"Frame {i} type differs"
        assert comp_frames[i][2] == frames[i][2], f"Frame {i} content differs"
    print(f"  All frame content identical")

    assert len(timemap) == len(frames), "Timemap size mismatch"
    print(f"  Timemap has {len(timemap)} entries")

    crlf_count = sum(1 for f in comp_frames if "\r\n" in f[2])
    print(f"  CR+LF frames: {crlf_count}/{len(comp_frames)}")

    print(f"\nDone! {raw_duration:.1f}s -> {compressed_duration:.1f}s ({ratio:.1f}x compression)")


if __name__ == "__main__":
    main()
