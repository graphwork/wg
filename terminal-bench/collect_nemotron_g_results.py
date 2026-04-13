#!/usr/bin/env python3
"""
Collect results from Harbor condition G run (Nemotron 3 Super) into the
standardized format for tb-results/nemotron-3-super-condition-g.json.

Also counts subtasks spawned per trial (key data for A-vs-G comparison).
"""

import json
import os
import sys
from pathlib import Path
from datetime import datetime, timezone

RESULTS_DIR = Path("results/nemotron-3-super-condition-G/nemotron-3-super-condition-G")
OUTPUT_FILE = Path("../tb-results/nemotron-3-super-condition-g.json")


def count_subtasks(trial_dir: Path) -> int:
    """Count subtasks by examining wg-artifacts in the trial directory."""
    agents_dirs = [
        trial_dir / "agent" / "wg-artifacts" / ".workgraph" / "agents",
        trial_dir / "wg-artifacts" / ".workgraph" / "agents",
        trial_dir / "artifacts" / "wg-artifacts" / ".workgraph" / "agents",
    ]
    for agents_dir in agents_dirs:
        if agents_dir.is_dir():
            return len(list(agents_dir.iterdir()))

    # Fallback: look at graph.jsonl for task count
    graph_paths = [
        trial_dir / "agent" / "wg-artifacts" / ".workgraph" / "graph.jsonl",
        trial_dir / "wg-artifacts" / ".workgraph" / "graph.jsonl",
        trial_dir / "artifacts" / "wg-artifacts" / ".workgraph" / "graph.jsonl",
    ]
    for graph_path in graph_paths:
        if graph_path.is_file():
            with open(graph_path) as f:
                count = sum(1 for line in f if line.strip())
            return max(count - 1, 0)  # subtract seed task

    return 0


def extract_model_from_stream(trial_dir: Path) -> str | None:
    """Extract actual model used from stream.jsonl files."""
    agents_dirs = [
        trial_dir / "agent" / "wg-artifacts" / ".workgraph" / "agents",
        trial_dir / "wg-artifacts" / ".workgraph" / "agents",
    ]
    for agents_dir in agents_dirs:
        if not agents_dir.is_dir():
            continue
        for agent_id in agents_dir.iterdir():
            stream = agent_id / "stream.jsonl"
            if not stream.is_file():
                continue
            with open(stream) as f:
                for line in f:
                    line = line.strip()
                    if not line:
                        continue
                    try:
                        event = json.loads(line)
                        if event.get("type") == "init":
                            return event.get("model")
                    except json.JSONDecodeError:
                        continue
    return None


def main():
    if not RESULTS_DIR.is_dir():
        print(f"Results directory not found: {RESULTS_DIR}")
        sys.exit(1)

    results = []
    total_passed = 0
    total_failed = 0
    total_errors = 0
    total_subtasks = 0

    for trial_dir in sorted(RESULTS_DIR.iterdir()):
        if not trial_dir.is_dir():
            continue

        result_file = trial_dir / "result.json"
        if not result_file.is_file():
            continue

        trial_name = trial_dir.name
        # Extract task name (strip the __hash suffix)
        task_name = trial_name.rsplit("__", 1)[0] if "__" in trial_name else trial_name

        try:
            with open(result_file) as f:
                result_data = json.load(f)
        except json.JSONDecodeError:
            total_errors += 1
            results.append({
                "task": task_name,
                "trial_id": trial_name,
                "status": "error",
                "reward": 0.0,
                "error_summary": "Failed to parse result.json",
                "subtask_count": 0,
                "wall_clock_s": 0.0,
            })
            continue

        # Get reward (1.0 = pass, 0.0 = fail)
        reward = result_data.get("reward", 0.0)
        if reward is None:
            reward = 0.0

        # Count subtasks
        subtask_count = count_subtasks(trial_dir)
        total_subtasks += subtask_count

        # Determine status
        if reward == 1.0:
            status = "pass"
            total_passed += 1
        elif result_data.get("stats", {}).get("n_errors", 0) > 0:
            status = "error"
            total_errors += 1
        else:
            status = "fail"
            total_failed += 1

        # Extract wall clock time
        started = result_data.get("started_at", "")
        finished = result_data.get("finished_at", "")
        wall_clock = 0.0
        if started and finished:
            try:
                t0 = datetime.fromisoformat(started)
                t1 = datetime.fromisoformat(finished)
                wall_clock = (t1 - t0).total_seconds()
            except (ValueError, TypeError):
                pass

        # Check for exception
        exception_file = trial_dir / "exception.txt"
        error_summary = ""
        if exception_file.is_file():
            with open(exception_file) as f:
                error_summary = f.read().strip()[:500]

        # Check actual model used
        actual_model = extract_model_from_stream(trial_dir)

        entry = {
            "task": task_name,
            "trial_id": trial_name,
            "status": status,
            "reward": reward,
            "subtask_count": subtask_count,
            "wall_clock_s": round(wall_clock, 2),
        }
        if error_summary:
            entry["error_summary"] = error_summary
        if actual_model:
            entry["actual_model"] = actual_model

        results.append(entry)

    total = len(results)
    pass_rate = total_passed / total if total > 0 else 0.0
    avg_subtasks = total_subtasks / total if total > 0 else 0.0

    output = {
        "run_id": "nemotron-3-super-condition-G",
        "condition": "G",
        "model": "openrouter:nvidia/nemotron-3-super-120b-a12b:free",
        "timestamp": datetime.now(timezone.utc).isoformat(),
        "n_concurrent": 5,
        "total_tasks": total,
        "passed": total_passed,
        "failed": total_failed,
        "errors": total_errors,
        "pass_rate": pass_rate,
        "avg_subtask_count": round(avg_subtasks, 2),
        "total_subtasks": total_subtasks,
        "tasks": results,
    }

    OUTPUT_FILE.parent.mkdir(parents=True, exist_ok=True)
    with open(OUTPUT_FILE, "w") as f:
        json.dump(output, f, indent=2)

    print(f"Results written to {OUTPUT_FILE}")
    print(f"Total: {total} | Pass: {total_passed} | Fail: {total_failed} | Error: {total_errors}")
    print(f"Pass rate: {pass_rate:.1%}")
    print(f"Avg subtasks: {avg_subtasks:.1f}")


if __name__ == "__main__":
    main()
